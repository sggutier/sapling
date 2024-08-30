/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This software may be used and distributed according to the terms of the
 * GNU General Public License version 2.
 */

use std::collections::HashMap;
use std::collections::HashSet;
use std::fmt::Debug;
use std::hash::Hash;

use anyhow::Error;
use blobstore::StoreLoadable;
use cloned::cloned;
use context::CoreContext;
use futures::future;
use futures::future::try_join_all;
use futures::try_join;
use futures::FutureExt;
use futures::Stream;
use futures::TryFutureExt;
use futures::TryStreamExt;
use maplit::hashmap;
use maplit::hashset;
use mononoke_types::fsnode::FsnodeFile;
use mononoke_types::ContentId;
use mononoke_types::FileType;
use mononoke_types::NonRootMPath;
use tokio::task;

use crate::AsyncManifest;
use crate::Entry;

pub(crate) type BonsaiEntry<ManifestId, FileId> = Entry<ManifestId, (FileType, FileId)>;

#[derive(Clone, Eq, Debug, Hash, PartialEq, PartialOrd, Ord)]
pub enum BonsaiDiffFileChange<FileId> {
    /// This file was changed (was added or modified) in this changeset.
    Changed(NonRootMPath, FileType, FileId),

    /// The file was marked changed, but one of the parent file ID was reused. This can happen in
    /// these situations:
    ///
    /// 1. The file type was changed without a corresponding change in file contents.
    /// 2. There's a merge and one of the parent nodes was picked as the resolution.
    ///
    /// This is separate from `Changed` because in these instances, if copy information is part of
    /// the node it wouldn't be recorded.
    ChangedReusedId(NonRootMPath, FileType, FileId),

    /// This file was deleted in this changeset.
    Deleted(NonRootMPath),
}

impl<FileId> BonsaiDiffFileChange<FileId> {
    pub fn path(&self) -> &NonRootMPath {
        match self {
            Self::Changed(path, ..) | Self::ChangedReusedId(path, ..) | Self::Deleted(path) => path,
        }
    }
    pub fn file_id(&self) -> Option<&FileId> {
        match self {
            Self::Changed(_, _, fid) | Self::ChangedReusedId(_, _, fid) => Some(fid),
            Self::Deleted(_) => None,
        }
    }
    pub fn file_type(&self) -> Option<&FileType> {
        match self {
            Self::Changed(_, ft, _) | Self::ChangedReusedId(_, ft, _) => Some(ft),
            Self::Deleted(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CompositeEntry<ManifestId, FileId>
where
    FileId: Hash + Eq,
    ManifestId: Hash + Eq,
{
    manifests: HashSet<ManifestId>,
    files: HashSet<(FileType, FileId)>,
}

impl<ManifestId, FileId> CompositeEntry<ManifestId, FileId>
where
    FileId: Hash + Eq,
    ManifestId: Hash + Eq,
{
    fn empty() -> Self {
        Self {
            manifests: hashset! {},
            files: hashset! {},
        }
    }

    fn insert(&mut self, entry: BonsaiEntry<ManifestId, FileId>) {
        match entry {
            Entry::Leaf(new_id) => {
                self.files.insert(new_id);
            }
            Entry::Tree(new_id) => {
                self.manifests.insert(new_id);
            }
        }
    }

    fn into_parts(
        self,
    ) -> (
        Option<HashSet<ManifestId>>,
        Option<HashSet<(FileType, FileId)>>,
    ) {
        let manifests = if self.manifests.is_empty() {
            None
        } else {
            Some(self.manifests)
        };

        let files = if self.files.is_empty() {
            None
        } else {
            Some(self.files)
        };

        (manifests, files)
    }
}

/// WorkEntry describes the work to be performed by the bounded_traversal to produce a Bonsai diff.
/// It maps a path to consider to the contents of the Manifest for which to produce Bonsai changes
/// at this path and the contents of the parents at this path.
type WorkEntry<ManifestId, FileId> = HashMap<
    NonRootMPath,
    (
        Option<BonsaiEntry<ManifestId, FileId>>,
        CompositeEntry<ManifestId, FileId>,
    ),
>;

/// Identify further work to be performed for this Bonsai diff.
async fn recurse_trees<ManifestId, FileId, Store, Leaf>(
    ctx: &CoreContext,
    store: &Store,
    path: Option<&NonRootMPath>,
    node: Option<ManifestId>,
    parents: HashSet<ManifestId>,
) -> Result<WorkEntry<ManifestId, FileId>, Error>
where
    Store: Send + Sync + Clone + 'static,
    FileId: Hash + Eq,
    ManifestId: Hash + Eq + StoreLoadable<Store>,
    <ManifestId as StoreLoadable<Store>>::Value:
        AsyncManifest<Store, TreeId = ManifestId, LeafId = Leaf> + Send + Sync,
    Leaf: ManifestLeafId<FileId = FileId>,
{
    // If there is a single parent, and it's unchanged, then we don't need to recurse.
    if parents.len() == 1 && node.as_ref() == parents.iter().next() {
        return Ok(HashMap::new());
    }

    let node = async {
        let r = match node {
            Some(node) => Some(node.load(ctx, store).await?),
            None => None,
        };
        Ok(r)
    };

    let parents = try_join_all(
        parents
            .into_iter()
            .map(|id| async move { id.load(ctx, store).await }),
    );

    let (node, parents) = try_join!(node, parents)?;

    let mut ret = HashMap::new();

    let convert_entry = |entry: Entry<ManifestId, Leaf>| match entry {
        Entry::Leaf(manif_leaf) => Entry::Leaf((manif_leaf.file_type(), manif_leaf.file_id())),
        Entry::Tree(mid) => Entry::Tree(mid),
    };

    if let Some(node) = node {
        let mut list = node.list(ctx, store).await?;
        while let Some((path, entry)) = list.try_next().await? {
            ret.entry(path).or_insert((None, CompositeEntry::empty())).0 =
                Some(convert_entry(entry));
        }
    }

    for parent in parents {
        let mut list = parent.list(ctx, store).await?;
        while let Some((path, entry)) = list.try_next().await? {
            ret.entry(path)
                .or_insert((None, CompositeEntry::empty()))
                .1
                .insert(convert_entry(entry));
        }
    }

    let ret = ret
        .into_iter()
        .map(|(p, e)| {
            let p = NonRootMPath::join_opt_element(path, &p);
            (p, e)
        })
        .collect();

    Ok(ret)
}

fn resolve_file_over_files<FileId>(
    path: NonRootMPath,
    node: (FileType, FileId),
    parents: HashSet<(FileType, FileId)>,
) -> Option<BonsaiDiffFileChange<FileId>>
where
    FileId: Hash + Eq,
{
    if parents.len() == 1 && parents.contains(&node) {
        return None;
    }

    Some(resolve_file_over_mixed(path, node, parents))
}

fn resolve_file_over_mixed<FileId>(
    path: NonRootMPath,
    node: (FileType, FileId),
    parents: HashSet<(FileType, FileId)>,
) -> BonsaiDiffFileChange<FileId>
where
    FileId: Hash + Eq,
{
    if parents.iter().any(|e| e.1 == node.1) {
        return BonsaiDiffFileChange::ChangedReusedId(path, node.0, node.1);
    }

    BonsaiDiffFileChange::Changed(path, node.0, node.1)
}

async fn bonsai_diff_unfold<ManifestId, FileId, Store, Leaf>(
    ctx: &CoreContext,
    store: &Store,
    path: NonRootMPath,
    node: Option<BonsaiEntry<ManifestId, FileId>>,
    parents: CompositeEntry<ManifestId, FileId>,
) -> Result<
    (
        Option<BonsaiDiffFileChange<FileId>>,
        WorkEntry<ManifestId, FileId>,
    ),
    Error,
>
where
    Store: Send + Sync + Clone + 'static,
    FileId: Hash + Eq,
    ManifestId: Hash + Eq + StoreLoadable<Store>,
    <ManifestId as StoreLoadable<Store>>::Value:
        AsyncManifest<Store, TreeId = ManifestId, LeafId = Leaf> + Send + Sync,
    Leaf: ManifestLeafId<FileId = FileId>,
{
    let res = match node {
        Some(Entry::Tree(node)) => {
            let (manifests, files) = parents.into_parts();

            // We have a tree here, so we'll need to compare it to the matching manifests in the parent
            // manifests.
            let recurse = recurse_trees(
                ctx,
                store,
                Some(&path),
                Some(node),
                manifests.unwrap_or_default(),
            )
            .await?;

            // If the parent manifests contained a file at the path where we have a tree, then we
            // must indicate that this path is being deleted, since otherwise we'd have an invalid
            // Bonsai changeset that contains files that are children of a file!
            let change = if files.is_some() {
                Some(BonsaiDiffFileChange::Deleted(path))
            } else {
                None
            };

            (change, recurse)
        }
        Some(Entry::Leaf(node)) => {
            // We have a file here. We won't need to recurse into the parents: the presence of this
            // file implicitly deletes all descendent files (if any exist).
            let recurse = hashmap! {};

            let change = match parents.into_parts() {
                // At least one of our parents has a manifest here. We must emit a file to delete
                // it.
                (Some(_trees), Some(files)) => Some(resolve_file_over_mixed(path, node, files)),
                // Our parents have only files at this path. We might not need to emit anything if
                // they have the same files.
                (None, Some(files)) => resolve_file_over_files(path, node, files),
                // We don't have files in the parents. Regardless of whether we have manifests, we'll
                // need to emit this file, so let's do so.
                (_, None) => Some(BonsaiDiffFileChange::Changed(path, node.0, node.1)),
            };

            (change, recurse)
        }
        None => {
            // We don't have anything at this path, but our parents do: we need to recursively
            // delete everything in this tree.
            let (manifests, files) = parents.into_parts();

            let recurse =
                recurse_trees(ctx, store, Some(&path), None, manifests.unwrap_or_default()).await?;

            let change = if files.is_some() {
                Some(BonsaiDiffFileChange::Deleted(path))
            } else {
                None
            };

            (change, recurse)
        }
    };

    Ok(res)
}

pub fn bonsai_diff<ManifestId, FileId, Store, Leaf>(
    ctx: CoreContext,
    store: Store,
    node: ManifestId,
    parents: HashSet<ManifestId>,
) -> impl Stream<Item = Result<BonsaiDiffFileChange<FileId>, Error>>
where
    FileId: Hash + Eq + Send + Sync + Clone + 'static,
    ManifestId: Hash + Eq + StoreLoadable<Store> + Send + Sync + 'static,
    Store: Send + Sync + Clone + 'static,
    <ManifestId as StoreLoadable<Store>>::Value:
        AsyncManifest<Store, TreeId = ManifestId, LeafId = Leaf> + Send + Sync,
    Leaf: ManifestLeafId<FileId = FileId>,
{
    // NOTE: `async move` blocks are used below to own CoreContext and Store for the (static)
    // lifetime of the stream we're returning here (recurse_trees and bonsai_diff_unfold don't
    // require those).
    {
        cloned!(ctx, store);
        async move { recurse_trees(&ctx, &store, None, Some(node), parents).await }
    }
    .map_ok(|seed| {
        bounded_traversal::bounded_traversal_stream(256, seed, move |(path, (node, parents))| {
            cloned!(ctx, store);
            async move {
                task::spawn(
                    async move { bonsai_diff_unfold(&ctx, &store, path, node, parents).await },
                )
                .await?
            }
            .boxed()
        })
    })
    .try_flatten_stream()
    .try_filter_map(future::ok)
}

/// Trait that generalizes the LeafId bound from `bonsai_diff` so that it can be
/// called with manifests other than the ones that have the leaf `(FileType, _)`.
pub trait ManifestLeafId: Hash + Eq {
    type FileId: Hash + Eq;

    fn file_type(&self) -> FileType;
    fn file_id(self) -> Self::FileId;
}

impl<FId: Hash + Eq> ManifestLeafId for (FileType, FId) {
    type FileId = FId;

    fn file_type(&self) -> FileType {
        self.0
    }

    fn file_id(self) -> Self::FileId {
        self.1
    }
}

impl ManifestLeafId for FsnodeFile {
    type FileId = (ContentId, u64);

    fn file_type(&self) -> FileType {
        *self.file_type()
    }

    fn file_id(self) -> Self::FileId {
        (*self.content_id(), self.size())
    }
}

#[cfg(test)]
mod test {
    use std::sync::Arc;

    use blobstore::Blobstore;
    use blobstore::Storable;
    use fbinit::FacebookInit;
    use memblob::Memblob;
    use mononoke_macros::mononoke;

    use super::*;
    use crate::tests::test_manifest::TestLeaf;
    use crate::tests::test_manifest::TestManifest;
    use crate::tests::test_manifest::TestManifestId;

    impl<ManifestId, FileId> CompositeEntry<ManifestId, FileId>
    where
        FileId: Hash + Eq,
        ManifestId: Hash + Eq,
    {
        fn files(files: HashSet<(FileType, FileId)>) -> Self {
            Self {
                manifests: hashset! {},
                files,
            }
        }

        fn manifests(manifests: HashSet<ManifestId>) -> Self {
            Self {
                manifests,
                files: hashset! {},
            }
        }
    }

    #[mononoke::fbinit_test]
    async fn test_unfold_file_from_files(fb: FacebookInit) -> Result<(), Error> {
        let ctx = CoreContext::test_mock(fb);
        let store: Arc<dyn Blobstore> = Arc::new(Memblob::default());
        let root = NonRootMPath::new("a")?;

        let leaf_id = TestLeaf::new("1").store(&ctx, &store).await?;
        let node = Some(BonsaiEntry::Leaf((FileType::Regular, leaf_id)));
        let mut parents: CompositeEntry<TestManifestId, _> = CompositeEntry::empty();

        // Start with no parent. The file should be added.
        let (change, work) =
            bonsai_diff_unfold(&ctx, &store, root.clone(), node, parents.clone()).await?;
        assert_eq!(
            change,
            Some(BonsaiDiffFileChange::Changed(
                root.clone(),
                FileType::Regular,
                leaf_id
            ))
        );
        assert_eq!(work, hashmap! {});

        // Add the same file in a parent
        parents.insert(BonsaiEntry::Leaf((FileType::Regular, leaf_id)));
        let (change, work) =
            bonsai_diff_unfold(&ctx, &store, root.clone(), node, parents.clone()).await?;
        assert_eq!(change, None);
        assert_eq!(work, hashmap! {});

        // Add the file again in a different parent
        parents.insert(BonsaiEntry::Leaf((FileType::Regular, leaf_id)));
        let (change, work) =
            bonsai_diff_unfold(&ctx, &store, root.clone(), node, parents.clone()).await?;
        assert_eq!(change, None);
        assert_eq!(work, hashmap! {});

        // Add a different file
        let leaf2_id = TestLeaf::new("2").store(&ctx, &store).await?;
        parents.insert(BonsaiEntry::Leaf((FileType::Regular, leaf2_id)));
        let (change, work) =
            bonsai_diff_unfold(&ctx, &store, root.clone(), node, parents.clone()).await?;
        assert_eq!(
            change,
            Some(BonsaiDiffFileChange::ChangedReusedId(
                root.clone(),
                FileType::Regular,
                leaf_id,
            ))
        );
        assert_eq!(work, hashmap! {});

        Ok(())
    }

    #[mononoke::fbinit_test]
    async fn test_unfold_file_mode_change(fb: FacebookInit) -> Result<(), Error> {
        let ctx = CoreContext::test_mock(fb);
        let store: Arc<dyn Blobstore> = Arc::new(Memblob::default());
        let root = NonRootMPath::new("a")?;

        let leaf_id = TestLeaf::new("1").store(&ctx, &store).await?;
        let node = Some(BonsaiEntry::Leaf((FileType::Regular, leaf_id)));
        let mut parents: CompositeEntry<TestManifestId, _> = CompositeEntry::empty();

        // Add a parent with a different mode. We can reuse it.
        parents.insert(BonsaiEntry::Leaf((FileType::Executable, leaf_id)));

        let (change, work) = bonsai_diff_unfold(&ctx, &store, root.clone(), node, parents).await?;
        assert_eq!(
            change,
            Some(BonsaiDiffFileChange::ChangedReusedId(
                root.clone(),
                FileType::Regular,
                leaf_id,
            ))
        );
        assert_eq!(work, hashmap! {});

        Ok(())
    }

    #[mononoke::fbinit_test]
    async fn test_unfold_file_from_dirs(fb: FacebookInit) -> Result<(), Error> {
        let ctx = CoreContext::test_mock(fb);
        let store: Arc<dyn Blobstore> = Arc::new(Memblob::default());
        let root = NonRootMPath::new("a")?;

        let leaf_id = TestLeaf::new("1").store(&ctx, &store).await?;
        let node = Some(BonsaiEntry::Leaf((FileType::Regular, leaf_id)));
        let mut parents: CompositeEntry<TestManifestId, _> = CompositeEntry::empty();

        let tree_id = TestManifest::new().store(&ctx, &store).await?;

        // Add a conflicting directory. We need to delete it.
        parents.insert(BonsaiEntry::Tree(tree_id));
        let (change, work) =
            bonsai_diff_unfold(&ctx, &store, root.clone(), node, parents.clone()).await?;
        assert_eq!(
            change,
            Some(BonsaiDiffFileChange::Changed(
                root.clone(),
                FileType::Regular,
                leaf_id
            ))
        );
        assert_eq!(work, hashmap! {});

        // Add another parent with the same file. We can reuse it but we still need to emit it.
        parents.insert(BonsaiEntry::Leaf((FileType::Regular, leaf_id)));
        let (change, work) =
            bonsai_diff_unfold(&ctx, &store, root.clone(), node, parents.clone()).await?;
        assert_eq!(
            change,
            Some(BonsaiDiffFileChange::ChangedReusedId(
                root.clone(),
                FileType::Regular,
                leaf_id,
            ))
        );
        assert_eq!(work, hashmap! {});

        // Add a different file. Same as above.
        let leaf2_id = TestLeaf::new("2").store(&ctx, &store).await?;
        parents.insert(BonsaiEntry::Leaf((FileType::Regular, leaf2_id)));
        let (change, work) =
            bonsai_diff_unfold(&ctx, &store, root.clone(), node, parents.clone()).await?;
        assert_eq!(
            change,
            Some(BonsaiDiffFileChange::ChangedReusedId(
                root.clone(),
                FileType::Regular,
                leaf_id,
            ))
        );
        assert_eq!(work, hashmap! {});

        Ok(())
    }

    #[mononoke::fbinit_test]
    async fn test_unfold_dir_from_dirs(fb: FacebookInit) -> Result<(), Error> {
        let ctx = CoreContext::test_mock(fb);
        let store: Arc<dyn Blobstore> = Arc::new(Memblob::default());
        let root = NonRootMPath::new("a")?;

        let leaf1_id = TestLeaf::new("1").store(&ctx, &store).await?;
        let leaf2_id = TestLeaf::new("2").store(&ctx, &store).await?;
        let tree1_id = TestManifest::new()
            .insert("p1", Entry::Leaf((FileType::Regular, leaf1_id)))
            .store(&ctx, &store)
            .await?;
        let tree2_id = TestManifest::new()
            .insert("p1", Entry::Leaf((FileType::Executable, leaf2_id)))
            .insert("p2", Entry::Tree(tree1_id))
            .store(&ctx, &store)
            .await?;

        let node = Some(Entry::Tree(tree1_id));
        let mut parents = CompositeEntry::empty();

        // No parents. We need to recurse in this directory.
        let (change, work) =
            bonsai_diff_unfold(&ctx, &store, root.clone(), node, parents.clone()).await?;
        assert_eq!(change, None);
        assert_eq!(
            work,
            hashmap! {
                NonRootMPath::new("a/p1")? => (Some(Entry::Leaf((FileType::Regular, leaf1_id))), CompositeEntry::empty()),
            }
        );

        // Identical parent. We are done.
        parents.insert(Entry::Tree(tree1_id));
        let (change, work) =
            bonsai_diff_unfold(&ctx, &store, root.clone(), node, parents.clone()).await?;
        assert_eq!(change, None);
        assert_eq!(work, hashmap! {});

        // One parent differs. Recurse on the 2 paths reachable from those manifests, and with the
        // contents listed in both parent manifests.
        parents.insert(Entry::Tree(tree2_id));
        let (change, work) =
            bonsai_diff_unfold(&ctx, &store, root.clone(), node, parents.clone()).await?;
        assert_eq!(change, None);
        assert_eq!(
            work,
            hashmap! {
                NonRootMPath::new("a/p1")? => (
                    Some(Entry::Leaf((FileType::Regular, leaf1_id))),
                    CompositeEntry::files(hashset! {
                        (FileType::Regular, leaf1_id),
                        (FileType::Executable, leaf2_id)
                    })
                ),
                NonRootMPath::new("a/p2")? => (
                    None,
                    CompositeEntry::manifests(hashset! { tree1_id })
                ),
            }
        );

        Ok(())
    }

    #[mononoke::fbinit_test]
    async fn test_unfold_dir_from_files(fb: FacebookInit) -> Result<(), Error> {
        let ctx = CoreContext::test_mock(fb);
        let store: Arc<dyn Blobstore> = Arc::new(Memblob::default());
        let root = NonRootMPath::new("a")?;

        let tree_id = TestManifest::new().store(&ctx, &store).await?;

        let node = Some(BonsaiEntry::Tree(tree_id));

        let mut parents = CompositeEntry::empty();

        // Parent has a file. Delete it.
        let leaf_id = TestLeaf::new("1").store(&ctx, &store).await?;
        parents.insert(BonsaiEntry::Leaf((FileType::Regular, leaf_id)));

        let (change, work) =
            bonsai_diff_unfold(&ctx, &store, root.clone(), node, parents.clone()).await?;
        assert_eq!(change, Some(BonsaiDiffFileChange::Deleted(root)));

        assert_eq!(work, hashmap! {});

        Ok(())
    }

    #[mononoke::fbinit_test]
    async fn test_unfold_missing_from_files(fb: FacebookInit) -> Result<(), Error> {
        let ctx = CoreContext::test_mock(fb);
        let store: Arc<dyn Blobstore> = Arc::new(Memblob::default());
        let root = NonRootMPath::new("a")?;

        let mut parents: CompositeEntry<TestManifestId, _> = CompositeEntry::empty();

        // Parent has a file, delete it.
        let leaf_id = TestLeaf::new("1").store(&ctx, &store).await?;
        parents.insert(BonsaiEntry::Leaf((FileType::Executable, leaf_id)));

        let (change, work) =
            bonsai_diff_unfold(&ctx, &store, root.clone(), None, parents.clone()).await?;
        assert_eq!(change, Some(BonsaiDiffFileChange::Deleted(root)));
        assert_eq!(work, hashmap! {});

        Ok(())
    }

    #[mononoke::fbinit_test]
    async fn test_unfold_missing_from_dirs(fb: FacebookInit) -> Result<(), Error> {
        let ctx = CoreContext::test_mock(fb);
        let store: Arc<dyn Blobstore> = Arc::new(Memblob::default());
        let root = NonRootMPath::new("a")?;

        let tree3_id = TestManifest::new().store(&ctx, &store).await?;
        let tree2_id = TestManifest::new()
            .insert("p2", Entry::Tree(tree3_id))
            .store(&ctx, &store)
            .await?;
        let tree1_id = TestManifest::new()
            .insert("p1", Entry::Tree(tree2_id))
            .store(&ctx, &store)
            .await?;

        let mut parents = CompositeEntry::empty();

        // Parent has a directory, recurse into it.
        parents.insert(BonsaiEntry::Tree(tree1_id));
        let (change, work) =
            bonsai_diff_unfold(&ctx, &store, root.clone(), None, parents.clone()).await?;
        assert_eq!(change, None);
        assert_eq!(
            work,
            hashmap! {
                NonRootMPath::new("a/p1")? => (
                    None,
                    CompositeEntry::manifests(hashset! { tree2_id })
                ),
            }
        );

        // Multiple parents have multiple directories. Recurse into all of them.
        parents.insert(BonsaiEntry::Tree(tree2_id));
        let (change, work) =
            bonsai_diff_unfold(&ctx, &store, root.clone(), None, parents.clone()).await?;
        assert_eq!(change, None);
        assert_eq!(
            work,
            hashmap! {
                NonRootMPath::new("a/p1")? => (
                    None,
                    CompositeEntry::manifests(hashset! { tree2_id })
                ),
                NonRootMPath::new("a/p2")? => (
                    None,
                    CompositeEntry::manifests(hashset! { tree3_id })
                ),
            }
        );

        Ok(())
    }
}
