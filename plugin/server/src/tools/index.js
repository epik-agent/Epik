// The server's tools. A new tool is a module in this directory, listed here.

import epicStatus from './epic_status.js';
import ghRaw from './gh_raw.js';
import issueListBlockedBy from './issue_list_blocked_by.js';
import issueRemoveBlockedBy from './issue_remove_blocked_by.js';
import issueSetBlockedBy from './issue_set_blocked_by.js';
import labelCreate from './label_create.js';
import labelDelete from './label_delete.js';
import labelList from './label_list.js';
import runGet from './run_get.js';
import runList from './run_list.js';
import runLogs from './run_logs.js';

export const tools = [
  ghRaw,
  labelList,
  labelCreate,
  labelDelete,
  runList,
  runGet,
  runLogs,
  issueSetBlockedBy,
  issueRemoveBlockedBy,
  issueListBlockedBy,
  epicStatus,
];
