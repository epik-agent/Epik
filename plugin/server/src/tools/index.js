// The server's tools. A new tool is a module in this directory, listed here.

import ghRaw from './gh_raw.js';
import labelCreate from './label_create.js';
import labelDelete from './label_delete.js';
import labelList from './label_list.js';
import runGet from './run_get.js';
import runList from './run_list.js';
import runLogs from './run_logs.js';

export const tools = [ghRaw, labelList, labelCreate, labelDelete, runList, runGet, runLogs];
