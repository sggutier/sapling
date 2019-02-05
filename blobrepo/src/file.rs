// Copyright (c) 2004-present, Facebook, Inc.
// All Rights Reserved.
//
// This software may be used and distributed according to the terms of the
// GNU General Public License version 2 or any later version.

//! Plain files, symlinks

use crate::failure::{Error, FutureFailureErrorExt};
use futures::future::{self, Future};
use futures_ext::{BoxFuture, FutureExt};

use super::alias::get_sha256;

use mercurial::file;
use mercurial_types::manifest::{Content, Entry, Manifest, Type};
use mercurial_types::nodehash::HgEntryId;
use mercurial_types::{
    FileType, HgBlob, HgFileEnvelope, HgFileNodeId, HgManifestId, HgNodeHash, HgParents, MPath,
    MPathElement,
};
use mononoke_types::{hash::Sha256, ContentId, FileContents, MononokeId};

use blobstore::Blobstore;
use context::CoreContext;

use crate::errors::*;

use crate::manifest::{fetch_manifest_envelope, fetch_raw_manifest_bytes, BlobManifest};

use blob_changeset::RepoBlobstore;

#[derive(Clone)]
pub struct HgBlobEntry {
    blobstore: RepoBlobstore,
    name: Option<MPathElement>,
    id: HgEntryId,
    ty: Type,
}

impl PartialEq for HgBlobEntry {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.id == other.id && self.ty == other.ty
    }
}

impl Eq for HgBlobEntry {}

pub fn fetch_raw_filenode_bytes(
    ctx: CoreContext,
    blobstore: &RepoBlobstore,
    node_id: HgNodeHash,
) -> BoxFuture<HgBlob, Error> {
    fetch_file_envelope(ctx.clone(), blobstore, node_id)
        .and_then({
            let blobstore = blobstore.clone();
            move |envelope| {
                let envelope = envelope.into_mut();
                let file_contents_fut = fetch_file_contents(ctx, &blobstore, envelope.content_id);

                let mut metadata = envelope.metadata;
                if metadata.is_empty() {
                    file_contents_fut
                        .map(|contents| HgBlob::from(contents.into_bytes()))
                        .boxify()
                } else {
                    file_contents_fut
                        .map(move |contents| {
                            // The copy info and the blob have to be joined together.
                            // TODO (T30456231): avoid the copy
                            metadata.extend_from_slice(contents.into_bytes().as_ref());
                            HgBlob::from(metadata)
                        })
                        .boxify()
                }
            }
        })
        .from_err()
        .boxify()
}

pub fn fetch_file_content_from_blobstore(
    ctx: CoreContext,
    blobstore: &RepoBlobstore,
    node_id: HgNodeHash,
) -> impl Future<Item = FileContents, Error = Error> {
    fetch_file_envelope(ctx.clone(), blobstore, node_id).and_then({
        let blobstore = blobstore.clone();
        move |envelope| {
            let content_id = envelope.content_id();
            fetch_file_contents(ctx, &blobstore, content_id.clone())
        }
    })
}

pub fn fetch_file_size_from_blobstore(
    ctx: CoreContext,
    blobstore: &RepoBlobstore,
    node_id: HgFileNodeId,
) -> impl Future<Item = u64, Error = Error> {
    fetch_file_envelope(ctx, blobstore, node_id.into_nodehash())
        .map({ |envelope| envelope.content_size() })
}

pub fn fetch_file_content_id_from_blobstore(
    ctx: CoreContext,
    blobstore: &RepoBlobstore,
    node_id: HgFileNodeId,
) -> impl Future<Item = ContentId, Error = Error> {
    fetch_file_envelope(ctx, blobstore, node_id.into_nodehash())
        .map({ |envelope| *envelope.content_id() })
}

pub fn fetch_file_content_sha256_from_blobstore(
    ctx: CoreContext,
    blobstore: &RepoBlobstore,
    content_id: ContentId,
) -> impl Future<Item = Sha256, Error = Error> {
    fetch_file_contents(ctx, blobstore, content_id)
        .map(|file_content| get_sha256(&file_content.into_bytes()))
}

pub fn fetch_rename_from_blobstore(
    ctx: CoreContext,
    blobstore: &RepoBlobstore,
    node_id: HgNodeHash,
) -> impl Future<Item = Option<(MPath, HgNodeHash)>, Error = Error> {
    fetch_file_envelope(ctx, blobstore, node_id).and_then(get_rename_from_envelope)
}

pub fn fetch_file_envelope(
    ctx: CoreContext,
    blobstore: &RepoBlobstore,
    node_id: HgNodeHash,
) -> impl Future<Item = HgFileEnvelope, Error = Error> {
    fetch_file_envelope_opt(ctx, blobstore, node_id)
        .and_then(move |envelope| {
            let envelope = envelope.ok_or(ErrorKind::HgContentMissing(
                node_id,
                Type::File(FileType::Regular),
            ))?;
            Ok(envelope)
        })
        .from_err()
}

pub fn fetch_file_envelope_opt(
    ctx: CoreContext,
    blobstore: &RepoBlobstore,
    node_id: HgNodeHash,
) -> impl Future<Item = Option<HgFileEnvelope>, Error = Error> {
    let blobstore_key = HgFileNodeId::new(node_id).blobstore_key();
    blobstore
        .get(ctx, blobstore_key.clone())
        .context("While fetching manifest envelope blob")
        .map_err(Error::from)
        .and_then(move |bytes| {
            let blobstore_bytes = match bytes {
                Some(bytes) => bytes,
                None => return Ok(None),
            };
            let envelope = HgFileEnvelope::from_blob(blobstore_bytes.into())?;
            if &node_id != envelope.node_id() {
                bail_msg!(
                    "Manifest ID mismatch (requested: {}, got: {})",
                    node_id,
                    envelope.node_id()
                );
            }
            Ok(Some(envelope))
        })
        .with_context(|_| ErrorKind::FileNodeDeserializeFailed(blobstore_key))
        .from_err()
}

pub fn fetch_file_contents(
    ctx: CoreContext,
    blobstore: &RepoBlobstore,
    content_id: ContentId,
) -> impl Future<Item = FileContents, Error = Error> {
    let blobstore_key = content_id.blobstore_key();
    blobstore
        .get(ctx, blobstore_key.clone())
        .context("While fetching content blob")
        .map_err(Error::from)
        .and_then(move |bytes| {
            let blobstore_bytes = match bytes {
                Some(bytes) => bytes,
                None => bail_err!(ErrorKind::ContentBlobMissing(content_id)),
            };
            let file_contents = FileContents::from_encoded_bytes(blobstore_bytes.into_bytes())?;
            Ok(file_contents)
        })
        .with_context(|_| ErrorKind::FileContentsDeserializeFailed(blobstore_key))
        .from_err()
}

pub(crate) fn get_rename_from_envelope(
    envelope: HgFileEnvelope,
) -> Result<Option<(MPath, HgNodeHash)>, Error> {
    let envelope = envelope.into_mut();

    // This is a bit of a hack because metadata is not the complete file. However, it's
    // equivalent to a zero-length file.
    file::File::new(
        envelope.metadata,
        envelope.p1.as_ref(),
        envelope.p2.as_ref(),
    )
    .copied_from()
}

impl HgBlobEntry {
    pub fn new(blobstore: RepoBlobstore, name: MPathElement, nodeid: HgNodeHash, ty: Type) -> Self {
        Self {
            blobstore,
            name: Some(name),
            id: HgEntryId::new(nodeid),
            ty,
        }
    }

    pub fn new_root(blobstore: RepoBlobstore, manifestid: HgManifestId) -> Self {
        Self {
            blobstore,
            name: None,
            id: HgEntryId::new(manifestid.into_nodehash()),
            ty: Type::Tree,
        }
    }

    fn get_raw_content_inner(&self, ctx: CoreContext) -> BoxFuture<HgBlob, Error> {
        match self.ty {
            Type::Tree => fetch_raw_manifest_bytes(ctx, &self.blobstore, self.id.into_nodehash()),
            Type::File(_) => {
                fetch_raw_filenode_bytes(ctx, &self.blobstore, self.id.into_nodehash())
            }
        }
    }
}

impl Entry for HgBlobEntry {
    fn get_type(&self) -> Type {
        self.ty
    }

    fn get_parents(&self, ctx: CoreContext) -> BoxFuture<HgParents, Error> {
        match self.ty {
            Type::Tree => {
                fetch_manifest_envelope(ctx.clone(), &self.blobstore, self.id.into_nodehash())
                    .map(move |envelope| {
                        let (p1, p2) = envelope.parents();
                        HgParents::new(p1, p2)
                    })
                    .boxify()
            }
            Type::File(_) => {
                fetch_file_envelope(ctx.clone(), &self.blobstore, self.id.into_nodehash())
                    .map(move |envelope| {
                        let (p1, p2) = envelope.parents();
                        HgParents::new(p1, p2)
                    })
                    .boxify()
            }
        }
    }

    fn get_raw_content(&self, ctx: CoreContext) -> BoxFuture<HgBlob, Error> {
        self.get_raw_content_inner(ctx)
    }

    fn get_content(&self, ctx: CoreContext) -> BoxFuture<Content, Error> {
        let blobstore = self.blobstore.clone();
        match self.ty {
            Type::Tree => {
                BlobManifest::load(ctx, &blobstore, &HgManifestId::new(self.id.into_nodehash()))
                    .and_then({
                        let node_id = self.id.into_nodehash();
                        move |blob_manifest| {
                            let manifest = blob_manifest
                                .ok_or(ErrorKind::HgContentMissing(node_id, Type::Tree))?;
                            Ok(Content::Tree(manifest.boxed()))
                        }
                    })
                    .context(format!(
                        "While HgBlobEntry::get_content for id {}, name {:?}, type {:?}",
                        self.id, self.name, self.ty
                    ))
                    .from_err()
                    .boxify()
            }
            Type::File(ft) => fetch_file_envelope(ctx.clone(), &blobstore, self.id.into_nodehash())
                .and_then(move |envelope| {
                    let envelope = envelope.into_mut();
                    let file_contents_fut =
                        fetch_file_contents(ctx, &blobstore, envelope.content_id);
                    file_contents_fut.map(move |contents| match ft {
                        FileType::Regular => Content::File(contents),
                        FileType::Executable => Content::Executable(contents),
                        FileType::Symlink => Content::Symlink(contents),
                    })
                })
                .context(format!(
                    "While HgBlobEntry::get_content for id {}, name {:?}, type {:?}",
                    self.id, self.name, self.ty
                ))
                .from_err()
                .boxify(),
        }
    }

    // XXX get_size should probably return a u64, not a usize
    fn get_size(&self, ctx: CoreContext) -> BoxFuture<Option<usize>, Error> {
        match self.ty {
            Type::Tree => future::ok(None).boxify(),
            Type::File(_) => {
                fetch_file_envelope(ctx.clone(), &self.blobstore, self.id.into_nodehash())
                    .map(|envelope| Some(envelope.content_size() as usize))
                    .boxify()
            }
        }
    }

    fn get_hash(&self) -> &HgEntryId {
        &self.id
    }

    fn get_name(&self) -> Option<&MPathElement> {
        self.name.as_ref()
    }
}
