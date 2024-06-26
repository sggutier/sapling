/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This software may be used and distributed according to the terms of the
 * GNU General Public License version 2.
 */

use anyhow::bail;
use anyhow::Context;
use anyhow::Result;
use async_trait::async_trait;
use bookmarks::BookmarkKey;
use bookmarks::BookmarkName;
use context::CoreContext;
use megarepo_configs::Source;
use megarepo_configs::SyncConfigVersion;
use mononoke_types::RepositoryId;
use sql_construct::SqlConstruct;
use sql_construct::SqlConstructFromMetadataDatabaseConfig;
use sql_ext::mononoke_queries;
use sql_ext::SqlConnections;

use crate::db::types::MegarepoSyncConfigEntry;
use crate::db::types::RowId;
use crate::db::MegarepoSyncConfig;

mononoke_queries! {
    write AddRepoConfig(repo_id: RepositoryId, bookmark: BookmarkName, version: SyncConfigVersion, serialized_config: &str) {
        none,
        "INSERT INTO megarepo_sync_config
        (`repo_id`, `bookmark`, `version`, `serialized_config`)
         VALUES ({repo_id}, {bookmark}, {version}, {serialized_config})
        "
    }

    read TestGetRepoConfigById(id: RowId)  -> (
        RowId,
        RepositoryId,
        BookmarkName,
        SyncConfigVersion,
        String,
    ) {
        "SELECT id, repo_id, bookmark, version, serialized_config
        FROM megarepo_sync_config
        WHERE id = {id}
        "
    }
}

fn row_to_entry(
    row: (RowId, RepositoryId, BookmarkName, SyncConfigVersion, String),
) -> Result<MegarepoSyncConfigEntry> {
    let (id, repo_id, bookmark, version, contents) = row;
    let sources: Vec<Source> = fbthrift::simplejson_protocol::deserialize(contents)
        .context("failed to deserialize existing config")?;
    Ok(MegarepoSyncConfigEntry {
        id,
        repo_id,
        bookmark,
        version,
        sources,
    })
}

#[derive(Clone)]
pub struct SqlMegarepoSyncConfig {
    pub(crate) connections: SqlConnections,
}

#[async_trait]
impl MegarepoSyncConfig for SqlMegarepoSyncConfig {
    async fn add_repo_config(
        &self,
        _ctx: &CoreContext,
        repo_id: &RepositoryId,
        bookmark: &BookmarkKey,
        version: &SyncConfigVersion,
        sources: Vec<Source>,
    ) -> Result<RowId> {
        let contents =
            String::from_utf8(fbthrift::simplejson_protocol::serialize(&sources).to_vec())
                .context("failed to serialize SyncTargetConfig")?;
        let res = AddRepoConfig::query(
            &self.connections.write_connection,
            repo_id,
            bookmark.name(),
            version,
            &contents.as_str(),
        )
        .await?;

        match res.last_insert_id() {
            Some(last_insert_id) if res.affected_rows() == 1 => Ok(RowId(last_insert_id)),
            _ => bail!(
                "Failed to insert a repo config for {} {} {}",
                repo_id,
                bookmark,
                version
            ),
        }
    }

    async fn test_get_repo_config_by_id(
        &self,
        _ctx: &CoreContext,
        id: &RowId,
    ) -> Result<Option<MegarepoSyncConfigEntry>> {
        let rows = TestGetRepoConfigById::query(&self.connections.read_connection, id).await?;
        match rows.into_iter().next() {
            None => Ok(None),
            Some(row) => Ok(Some(row_to_entry(row)?)),
        }
    }
}

impl SqlConstruct for SqlMegarepoSyncConfig {
    const LABEL: &'static str = "megarepo_sync_config";

    const CREATION_QUERY: &'static str =
        include_str!("../../schemas/sqlite-megarepo_sync_config.sql");

    fn from_sql_connections(connections: SqlConnections) -> Self {
        Self { connections }
    }
}

impl SqlConstructFromMetadataDatabaseConfig for SqlMegarepoSyncConfig {}

#[cfg(test)]
mod test {
    use fbinit::FacebookInit;

    use super::*;

    #[fbinit::test]
    async fn test_add_repo_config(fb: FacebookInit) -> Result<()> {
        let ctx = CoreContext::test_mock(fb);
        let config = SqlMegarepoSyncConfig::with_sqlite_in_memory()?;
        let id = config
            .add_repo_config(
                &ctx,
                &RepositoryId::new(0),
                &BookmarkKey::new("book")?,
                &SyncConfigVersion::new(),
                vec![],
            )
            .await?;

        let entry = config.test_get_repo_config_by_id(&ctx, &id).await?;
        assert!(entry.is_some());

        let entry = entry.unwrap();
        assert_eq!(entry.repo_id, RepositoryId::new(0));
        assert_eq!(entry.bookmark, *BookmarkKey::new("book")?.name());
        assert_eq!(entry.version, SyncConfigVersion::new());
        assert_eq!(entry.sources, vec![]);

        Ok(())
    }
}
