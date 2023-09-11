/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This software may be used and distributed according to the terms of the
 * GNU General Public License version 2.
 */

use std::path::Path;

use anyhow::Result;
use eden::GetScmStatusParams;
use eden::GetScmStatusResult;
use thrift_types::edenfs as eden;
use types::HgId;

use crate::client::EdenFsClient;

pub fn get_status(repo_root: &Path, commit: HgId, ignored: bool) -> Result<GetScmStatusResult> {
    let rt = tokio::runtime::Runtime::new()?;

    rt.block_on(get_status_internal(repo_root, commit, ignored))
}

async fn get_status_internal(
    repo_root: &Path,
    commit: HgId,
    ignored: bool,
) -> Result<GetScmStatusResult> {
    let client = EdenFsClient::from_wdir(repo_root)?;
    let thrift_client = client.get_thrift_client().await?;

    thrift_client
        .getScmStatusV2(&GetScmStatusParams {
            mountPoint: client.root().as_bytes().to_vec(),
            commit: commit.into_byte_array().into(),
            listIgnored: ignored,
            ..Default::default()
        })
        .await
        .map_err(|err| err.into())
}
