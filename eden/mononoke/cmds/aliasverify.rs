/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This software may be used and distributed according to the terms of the
 * GNU General Public License version 2.
 */

use std::cmp;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use anyhow::format_err;
use anyhow::Context;
use anyhow::Error;
use anyhow::Result;
use async_trait::async_trait;
use blobstore::Loadable;
use blobstore::Storable;
use bytes::Bytes;
use changesets::Changesets;
use changesets::ChangesetsRef;
use clap::ArgEnum;
use clap::Parser;
use context::CoreContext;
use executor_lib::RepoShardedProcess;
use executor_lib::RepoShardedProcessExecutor;
use executor_lib::ShardedProcessExecutor;
use fbinit::FacebookInit;
use filestore::hash_bytes;
use filestore::Alias;
use filestore::AliasBlob;
use filestore::Blake3IncrementalHasher;
use filestore::FetchKey;
use filestore::GitSha1IncrementalHasher;
use filestore::Sha1IncrementalHasher;
use filestore::Sha256IncrementalHasher;
use futures::future::TryFutureExt;
use futures::stream;
use futures::stream::StreamExt;
use futures::stream::TryStreamExt;
use mercurial_types::FileBytes;
use mononoke_app::args::AsRepoArg;
use mononoke_app::args::RepoArgs;
use mononoke_app::fb303::AliveService;
use mononoke_app::fb303::Fb303AppExtension;
use mononoke_app::MononokeApp;
use mononoke_app::MononokeAppBuilder;
use mononoke_app::MononokeReposManager;
use mononoke_repos::MononokeRepos;
use mononoke_types::ChangesetId;
use mononoke_types::ContentAlias;
use mononoke_types::ContentId;
use mononoke_types::FileChange;
use mutable_counters::MutableCounters;
use mutable_counters::MutableCountersRef;
use once_cell::sync::OnceCell;
use repo_blobstore::RepoBlobstore;
use repo_blobstore::RepoBlobstoreRef;
use repo_identity::RepoIdentity;
use slog::debug;
use slog::info;
use slog::Logger;

const LIMIT: usize = 1000;
const SM_SERVICE_SCOPE: &str = "global";
const SM_CLEANUP_TIMEOUT_SECS: u64 = 120;

#[facet::container]
pub struct Repo {
    #[facet]
    repo_identity: RepoIdentity,
    #[facet]
    repo_blobstore: RepoBlobstore,
    #[facet]
    mutable_counters: dyn MutableCounters,
    #[facet]
    changesets: dyn Changesets,
}

#[derive(Debug, Clone, Copy, ArgEnum)]
enum Mode {
    /// Mode to verify if the alias exists, and if it doesn't, report the error
    Verify,
    /// Mode to verify if the alias exists, and if it doesn't then generate it.
    Generate,
    /// Mode to generate aliases (along with metadata) for large collection of files.
    /// Can be used for backfilling repos with metadata and new aliases. In this mode,
    /// min_cs_db_id is ignored and {repo_name}_alias_backfill_counter mutable counter
    /// is used to determine the starting changeset for backfilling. If the mutable counter
    /// doesn't exist, the backfilling starts from cs_id 0.
    Backfill,
}

#[derive(Debug, Clone, Copy, ArgEnum)]
enum AliasType {
    Sha256,
    SeededBlake3,
    Sha1,
    GitSha1,
}

impl AliasType {
    fn get_alias(&self, content: &Bytes) -> Alias {
        match self {
            AliasType::GitSha1 => {
                Alias::GitSha1(hash_bytes(GitSha1IncrementalHasher::new(content), content).sha1())
            }
            AliasType::SeededBlake3 => {
                Alias::SeededBlake3(hash_bytes(Blake3IncrementalHasher::new_seeded(), content))
            }
            AliasType::Sha1 => Alias::Sha1(hash_bytes(Sha1IncrementalHasher::new(), content)),
            AliasType::Sha256 => Alias::Sha256(hash_bytes(Sha256IncrementalHasher::new(), content)),
        }
    }
}

/// Struct representing the Alias Verify process.
pub struct AliasVerifyProcess {
    app: MononokeApp,
    repos_mgr: MononokeReposManager<Repo>,
}

impl AliasVerifyProcess {
    pub fn new(app: MononokeApp, repos_mgr: MononokeReposManager<Repo>) -> Self {
        Self { app, repos_mgr }
    }
}

#[async_trait]
impl RepoShardedProcess for AliasVerifyProcess {
    async fn setup(&self, repo_name: &str) -> anyhow::Result<Arc<dyn RepoShardedProcessExecutor>> {
        let logger = self.app.repo_logger(repo_name);
        info!(&logger, "Setting up alias verify for repo {}", repo_name);
        let args: AliasVerifyArgs = self.app.args()?;
        let ctx = self.app.new_basic_context();
        self.repos_mgr
            .add_repo(repo_name.as_ref())
            .await
            .with_context(|| format!("Failure in opening repo {}", repo_name))?;
        Ok(Arc::new(AliasVerification::new(
            logger.clone(),
            repo_name.to_string(),
            self.repos_mgr.repos().clone(),
            args.mode,
            args.alias_type,
            Arc::new(AtomicBool::new(false)),
            args.step,
            args.min_cs_db_id,
            args.concurrency,
            ctx,
        )))
    }
}

/// Verify and reload all the alias blobs
#[derive(Parser)]
#[clap(about = "Verify and reload all the alias blobs into Mononoke blobstore.")]
struct AliasVerifyArgs {
    /// The type of alias to verify or generate (in case of missing alias)
    #[clap(long, arg_enum, default_value_t = AliasType::Sha256)]
    alias_type: AliasType,
    /// Mode for missing blobs
    #[clap(long, arg_enum, default_value_t = Mode::Verify)]
    mode: Mode,
    /// Number of commit ids to process at a time
    #[clap(long, default_value_t = 5000)]
    step: u64,
    /// Changeset to start verification from. Id from changeset table. Not connected to hash
    #[clap(long, default_value_t = 0)]
    min_cs_db_id: u64,
    /// Concurrency limit defining how many commits to be processed in parallel
    #[clap(long, default_value_t = LIMIT)]
    concurrency: usize,
    /// The repo against which the alias verify command needs to be executed
    #[clap(flatten)]
    repo: RepoArgs,
    /// The name of ShardManager service to be used when running alias verify in sharded setting.
    #[clap(long, conflicts_with_all = &["repo-name", "repo-id"])]
    pub sharded_service_name: Option<String>,
}

/// Struct representing the Alias Verify process over the context of Repo.
struct AliasVerification {
    logger: Logger,
    repos: Arc<MononokeRepos<Repo>>,
    repo_name: String,
    mode: Mode,
    alias_type: AliasType,
    step: u64,
    min_cs_db_id: u64,
    concurrency: usize,
    ctx: CoreContext,
    err_cnt: Arc<AtomicUsize>,
    cs_processed: Arc<AtomicUsize>,
    cancellation_requested: Arc<AtomicBool>,
}

#[async_trait]
impl RepoShardedProcessExecutor for AliasVerification {
    async fn execute(&self) -> anyhow::Result<()> {
        info!(
            self.logger,
            "Initiating alias verify execution for repo {}", &self.repo_name,
        );

        self.verify_all().await.with_context(|| {
            format!(
                "Error while executing alias verify for repo {}",
                &self.repo_name
            )
        })
    }

    async fn stop(&self) -> anyhow::Result<()> {
        info!(
            self.logger,
            "Terminating alias verify execution for repo {}", &self.repo_name,
        );
        self.cancellation_requested.store(true, Ordering::Relaxed);
        Ok(())
    }
}

impl AliasVerification {
    pub fn new(
        logger: Logger,
        repo_name: String,
        repos: Arc<MononokeRepos<Repo>>,
        mode: Mode,
        alias_type: AliasType,
        cancellation_requested: Arc<AtomicBool>,
        step: u64,
        min_cs_db_id: u64,
        concurrency: usize,
        ctx: CoreContext,
    ) -> Self {
        Self {
            logger,
            repo_name,
            repos,
            mode,
            alias_type,
            cancellation_requested,
            step,
            min_cs_db_id,
            concurrency,
            ctx,
            err_cnt: Arc::new(AtomicUsize::new(0)),
            cs_processed: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn repo(&self) -> Result<Arc<Repo>> {
        self.repos
            .get_by_name(self.repo_name.as_ref())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Requested repo {} does not exist on this server",
                    self.repo_name
                )
            })
    }

    async fn get_file_changes_vector(&self, bcs_id: ChangesetId) -> Result<Vec<FileChange>, Error> {
        let cs_cnt = self.cs_processed.fetch_add(1, Ordering::Relaxed);
        if cs_cnt % 1000 == 0 {
            info!(self.logger, "Commit processed {:?}", cs_cnt);
        }
        let bcs = bcs_id
            .load(&self.ctx, self.repo()?.repo_blobstore())
            .await?;
        let file_changes: Vec<_> = bcs
            .file_changes_map()
            .iter()
            .map(|(_path, fc)| fc.clone())
            .collect();
        Ok(file_changes)
    }

    async fn check_alias_blob(
        &self,
        alias: &Alias,
        expected_content_id: ContentId,
        content_id: ContentId,
    ) -> Result<(), Error> {
        if content_id == expected_content_id {
            // Everything is good
            Ok(())
        } else {
            panic!(
                "Collision: Wrong content_id by alias for {:?},
                ContentId in the blobstore {:?},
                Expected ContentId {:?}",
                alias, content_id, expected_content_id
            );
        }
    }

    async fn process_missing_alias_blob(
        &self,
        ctx: &CoreContext,
        alias: &Alias,
        content_id: ContentId,
    ) -> Result<(), Error> {
        self.err_cnt.fetch_add(1, Ordering::Relaxed);
        debug!(
            self.logger,
            "Missing alias blob: alias {}, content_id {:?}", alias, content_id
        );
        match self.mode {
            Mode::Verify => Ok(()),
            Mode::Generate | Mode::Backfill => {
                let blobstore = self.repo()?.repo_blobstore().clone();
                let maybe_meta =
                    filestore::get_metadata(&blobstore, ctx, &FetchKey::Canonical(content_id))
                        .await?;
                let meta =
                    maybe_meta.ok_or_else(|| format_err!("Missing content {:?}", content_id))?;
                let is_valid_match = match *alias {
                    Alias::Sha256(hash_val) => meta.sha256 == hash_val,
                    Alias::GitSha1(hash_val) => meta.git_sha1.sha1() == hash_val,
                    Alias::SeededBlake3(hash_val) => meta.seeded_blake3 == hash_val,
                    Alias::Sha1(hash_val) => meta.sha1 == hash_val,
                };
                if is_valid_match {
                    AliasBlob(alias.clone(), ContentAlias::from_content_id(content_id))
                        .store(ctx, &blobstore)
                        .await
                } else {
                    Err(format_err!(
                        "Inconsistent hashes for {:?}, got {:?}, metadata hashes are (Sha1: {:?}, Sha256: {:?}, GitSha1: {:?}, SeededBlake3: {:?})",
                        content_id,
                        alias,
                        meta.sha1,
                        meta.sha256,
                        meta.git_sha1.sha1(),
                        meta.seeded_blake3,
                    ))
                }
            }
        }
    }

    async fn process_alias(
        &self,
        ctx: &CoreContext,
        alias: &Alias,
        content_id: ContentId,
    ) -> Result<(), Error> {
        let result = FetchKey::from(alias.clone())
            .load(ctx, self.repo()?.repo_blobstore())
            .await;
        match result {
            Ok(content_id_from_blobstore) => {
                self.check_alias_blob(alias, content_id, content_id_from_blobstore)
                    .await
            }
            Err(_) => {
                // the blob with alias is not found
                self.process_missing_alias_blob(ctx, alias, content_id)
                    .await
            }
        }
    }

    pub async fn process_file_content(&self, content_id: ContentId) -> Result<(), Error> {
        let ctx = &self.ctx;
        let alias = filestore::fetch_concat(self.repo()?.repo_blobstore(), ctx, content_id)
            .map_ok(FileBytes)
            .map_ok(|content| self.alias_type.get_alias(&content.into_bytes()))
            .await?;
        self.process_alias(ctx, &alias, content_id).await
    }

    fn print_report(&self, partial: bool) {
        let resolution = if partial { "continues" } else { "finished" };
        info!(
            self.logger,
            "Alias Verification {}: {:?} errors found",
            resolution,
            self.err_cnt.load(Ordering::Relaxed)
        );
    }

    fn counter_name(&self) -> Result<String> {
        Ok(format!("{}_alias_backfill_counter", self.repo_name))
    }

    async fn get_bounded(&self, min_id: u64, max_id: u64) -> Result<(), Error> {
        if self.cancellation_requested.load(Ordering::Relaxed) {
            return Ok(());
        }
        info!(
            self.logger,
            "Process Changesets with ids: [{:?}, {:?})", min_id, max_id
        );
        let repo = self.repo()?;
        let bcs_ids = repo
            .changesets()
            .list_enumeration_range(&self.ctx, min_id, max_id, None, true);
        let count = AtomicUsize::new(0);
        let rcount = &count;
        let file_changes_stream = bcs_ids
            .and_then(move |(bcs_id, _)| async move {
                let file_changes_vec = self.get_file_changes_vector(bcs_id).await?;
                Ok(stream::iter(file_changes_vec).map(anyhow::Ok))
            })
            .try_flatten()
            .boxed();

        file_changes_stream
            .try_for_each_concurrent(self.concurrency, move |file_change| async move {
                rcount.fetch_add(1, Ordering::Relaxed);
                match file_change.simplify() {
                    Some(tc) => self.process_file_content(tc.content_id().clone()).await,
                    None => Ok(()),
                }
            })
            .await?;
        info!(
            self.logger,
            "Processed {} changesets",
            rcount.load(Ordering::Relaxed)
        );

        if let Mode::Backfill = self.mode {
            info!(
                self.logger,
                "Completed processing till changeset ID {}",
                max_id.to_string()
            );
            let counter_name = self.counter_name()?;
            repo.mutable_counters()
                .set_counter(&self.ctx, &counter_name, max_id as i64, None)
                .await
                .with_context(|| {
                    format!(
                        "Failed to set {} for {} to {}",
                        counter_name, self.repo_name, max_id
                    )
                })?;
        }
        self.print_report(true);
        Ok(())
    }

    pub async fn verify_all(&self) -> Result<(), Error> {
        let (ctx, step, min_cs_db_id, repo) =
            (&self.ctx, self.step, self.min_cs_db_id, self.repo()?);
        let (min_id, max_id) = repo
            .changesets()
            .enumeration_bounds(ctx, true, vec![])
            .await?
            .unwrap();
        let counter_name = self.counter_name()?;
        let init_changeset_id = match self.mode {
            Mode::Backfill => repo
                .mutable_counters()
                .get_counter(ctx, &counter_name)
                .await
                .with_context(|| format!("Error while getting mutable counter {}", counter_name))?
                .unwrap_or(0) as u64,
            _ => min_cs_db_id,
        };
        let mut bounds = vec![];
        let mut cur_id = cmp::max(min_id, init_changeset_id);
        info!(
            self.logger,
            "Initiating aliasverify in {:?} mode with input init changesetid {} and actual init changesetid {}. Max changesetid {}",
            self.mode,
            init_changeset_id,
            cur_id,
            max_id,
        );
        let max_id = max_id + 1;
        while cur_id < max_id {
            let max = cmp::min(max_id, cur_id + step);
            bounds.push((cur_id, max));
            cur_id += step;
        }
        stream::iter(bounds)
            .map(Ok)
            .try_for_each(move |(min_val, max_val)| self.get_bounded(min_val, max_val))
            .await?;
        self.print_report(false);
        Ok(())
    }
}

async fn async_main(app: MononokeApp) -> Result<(), Error> {
    let args: AliasVerifyArgs = app.args()?;
    // TODO(rajshar): Replace this None with ShardedService as AliasVerify
    let repo_mgr = app.open_managed_repos(None).await?;
    let process = AliasVerifyProcess::new(app, repo_mgr);
    match args.sharded_service_name {
        None => {
            let (repo_name, _) = process.app.repo_config(args.repo.as_repo_arg())?;
            let alias_verify = process.setup(repo_name.as_ref()).await?;
            alias_verify.execute().await
        }
        Some(name) => {
            let logger = process.app.logger().clone();
            // The service name needs to be 'static to satisfy SM contract
            static SM_SERVICE_NAME: OnceCell<String> = OnceCell::new();
            let mut executor = ShardedProcessExecutor::new(
                process.app.fb,
                process.app.runtime().clone(),
                &logger,
                SM_SERVICE_NAME.get_or_init(|| name),
                SM_SERVICE_SCOPE,
                SM_CLEANUP_TIMEOUT_SECS,
                Arc::new(process),
                true, // enable shard (repo) level healing
            )?;
            executor
                .block_and_execute(&logger, Arc::new(AtomicBool::new(false)))
                .await
        }
    }
}

#[fbinit::main]
fn main(fb: FacebookInit) -> Result<()> {
    let app = MononokeAppBuilder::new(fb)
        .with_app_extension(Fb303AppExtension {})
        .build::<AliasVerifyArgs>()?;
    app.run_with_monitoring_and_logging(async_main, "aliasverify", AliveService)
}
