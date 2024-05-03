/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This software may be used and distributed according to the terms of the
 * GNU General Public License version 2.
 */

use std::num::NonZeroU64;

use anyhow::anyhow;
use anyhow::Context;
use async_stream::try_stream;
use async_trait::async_trait;
use edenapi_types::SuffixQueryRequest;
use edenapi_types::SuffixQueryResponse;
use futures::StreamExt;
use gotham_ext::error::HttpError;
use itertools::EitherOrBoth;
use mononoke_api::ChangesetFileOrdering;
use types::RepoPathBuf;
use vec1::Vec1;

use super::handler::EdenApiContext;
use super::EdenApiHandler;
use super::EdenApiMethod;
use super::HandlerResult;
use crate::errors::ErrorKind;

pub struct SuffixQueryHandler;

#[async_trait]
impl EdenApiHandler for SuffixQueryHandler {
    type Request = SuffixQueryRequest;
    type Response = SuffixQueryResponse;

    const HTTP_METHOD: hyper::Method = hyper::Method::POST;
    const API_METHOD: EdenApiMethod = EdenApiMethod::SuffixQuery;
    const ENDPOINT: &'static str = "/suffix_query";

    fn sampling_rate(_request: &Self::Request) -> NonZeroU64 {
        nonzero_ext::nonzero!(100u64)
    }

    async fn handler(
        ectx: EdenApiContext<Self::PathExtractor, Self::QueryStringExtractor>,
        request: Self::Request,
    ) -> HandlerResult<'async_trait, Self::Response> {
        let repo = ectx.repo();
        let suffixes = Vec1::try_from_vec(request.basename_suffixes)
            .with_context(|| anyhow!("No suffixes provided"))
            .map_err(HttpError::e400)?;
        let commit = request.commit.clone();

        // Changeset may return None if given an incorrect commit id.
        let changeset = repo
            .repo()
            .changeset(commit.clone())
            .await
            .with_context(|| anyhow!("Error getting changeset {}", commit.clone()))?
            .ok_or_else(|| ErrorKind::CommitIdNotFound(commit.clone()))
            .map_err(HttpError::e400)?;

        Ok(try_stream! {
            // Find files may return None if BSSM tree does not exist(eg. testing locally)
            // Will cause server to return 500 error.
            let matched_files = changeset
                .find_files_with_bssm_v3(
                    None,
                    EitherOrBoth::Right(suffixes),
                    ChangesetFileOrdering::Unordered,
                ).await?;

            for await mpath in matched_files {
                let mpath = mpath?;
                let file_path = RepoPathBuf::from_string(mpath.to_string())?;
                yield SuffixQueryResponse {
                    file_path
                }
            }
        }
        .boxed())
    }
}
