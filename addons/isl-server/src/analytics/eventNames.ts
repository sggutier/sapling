/**
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is licensed under the MIT license found in the
 * LICENSE file in the root directory of this source tree.
 */

export type TrackEventName =
  | 'ClickedRefresh'
  | 'ClientConnection'
  | 'LoadMoreCommits'
  | 'RunOperation'
  | 'StarRating'
  | 'TopLevelErrorShown'
  | 'UIEmptyState'
  | 'HeadCommitChanged'
  | 'AbortMergeOperation'
  | 'PullOperation'
  | 'AbortMergeOperation'
  | 'AddOperation'
  | 'AddRemoveOperation'
  | 'AlertShown'
  | 'AlertDismissed'
  | 'AmendMessageOperation'
  | 'AmendOperation'
  | 'AmendFileSubsetOperation'
  | 'AmendToOperation'
  | 'ArcPullOperation'
  | 'BulkRebaseOperation'
  | 'CommitOperation'
  | 'CommitFileSubsetOperation'
  | 'ContinueMergeOperation'
  | 'CommitCloudStatusCommand'
  | 'CommitCloudListCommand'
  | 'CommitCloudSyncBackupStatusCommand'
  | 'CommitCloudChangeWorkspaceOperation'
  | 'CommitCloudCreateWorkspaceOperation'
  | 'CommitCloudSyncOperation'
  | 'CreateEmptyInitialCommit'
  | 'ClickSuggestedRebase'
  | 'DiscardOperation'
  | 'EnterMergeConflicts'
  | 'ExitMergeConflicts'
  | 'ForgetOperation'
  | 'FoldOperation'
  | 'FillCommitMessage'
  | 'GettingStartedInteraction'
  | 'GetSuggestedReviewers'
  | 'GetAlertsCommand'
  | 'AcceptSuggestedReviewer'
  | 'GenerateAICommitMessage'
  | 'GenerateAICommitMessageFunnelEvent'
  | 'GhStackSubmitOperation'
  | 'GotoOperation'
  | 'GoBackToOldISL'
  | 'GoBackToOldISLOnce'
  | 'GoBackToOldISLReason'
  | 'GraftOperation'
  | 'HideOperation'
  | 'ImportStackOperation'
  | 'LandModalOpen'
  | 'LandModalConfirm'
  | 'LandModalSuccess'
  | 'LandModalError'
  | 'LandModalUriLandShown'
  | 'LandModalCliLandShown'
  | 'LandRoadblockShown'
  | 'LandRoadblockContinue'
  | 'LandRoadblockContinueExternal'
  | 'LandSyncWarningShown'
  | 'LandSyncWarningChoseUseRemote'
  | 'LandSyncWarningChoseSyncLocal'
  | 'PartialCommitOperation'
  | 'PartialAmendOperation'
  | 'PartialDiscardOperation'
  | 'PrSubmitOperation'
  | 'PullOperation'
  | 'PullRevOperation'
  | 'PurgeOperation'
  | 'RebaseKeepOperation'
  | 'RebaseAllDraftCommitsOperation'
  | 'RebaseOperation'
  | 'ResolveOperation'
  | 'RevertOperation'
  | 'SetConfigOperation'
  | 'ShelveOperation'
  | 'UnshelveOperation'
  | 'RunCommand'
  | 'StatusCommand'
  | 'LogCommand'
  | 'LookupCommitsCommand'
  | 'LookupAllCommitChangedFilesCommand'
  | 'GetShelvesCommand'
  | 'GetConflictsCommand'
  | 'BlameCommand'
  | 'CatCommand'
  | 'DiffCommand'
  | 'FetchCommitTemplateCommand'
  | 'ImportStackCommand'
  | 'ExportStackCommand'
  | 'ShowBugButtonNux'
  | 'StackEditMetrics'
  | 'StackEditChangeTab'
  | 'StackEditInlineSplitButton'
  | 'SplitOpenFromCommitContextMenu'
  | 'SplitOpenFromHeadCommit'
  | 'SplitOpenRangeSelector'
  | 'SyncDiffMessageMutation'
  | 'ConfirmSyncNewDiffNumber'
  | 'UncommitOperation'
  | 'JfSubmitOperation'
  | 'JfGetOperation'
  | 'OptimisticFilesStateForceResolved'
  | 'OptimisticCommitsStateForceResolved'
  | 'OptimisticConflictsStateForceResolved'
  | 'OptInToNewISLAgain'
  | 'OpenAllFiles'
  | 'QueueOperation'
  | 'UploadImage'
  | 'RunVSCodeCommand'
  | 'RageCommand'
  | 'UnsubmittedStarRating'
  | 'BlameLoaded'
  | 'VSCodeExtensionActivated'
  | 'UseCustomCommitMessageTemplate';

export type TrackErrorName =
  | 'BlameError'
  | 'DiffFetchFailed'
  | 'InvalidCwd'
  | 'InvalidCommand'
  | 'JfNotAuthenticated'
  | 'GhCliNotAuthenticated'
  | 'GhCliNotInstalled'
  | 'LandModalError'
  | 'TopLevelError'
  | 'FetchError'
  | 'RunOperationError'
  | 'RunCommandError'
  | 'RepositoryError'
  | 'SyncMessageError'
  | 'UploadImageError'
  | 'VSCodeCommandError'
  | 'VSCodeActivationError';
