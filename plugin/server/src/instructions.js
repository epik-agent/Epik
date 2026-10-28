// What the server tells every session that has the plugin enabled. It stays a
// few sentences long; the substance lives in the `assistant` skill.
export const instructions =
  'Epik plans software with the user and has Claude Code build it from GitHub issues. ' +
  "Before you design, plan or scope software with the user, write or revise an epic or issue, or start a build, read Epik's `assistant` skill (`epik:assistant`): " +
  'it holds what Epik believes about building software and how Epik writes epics. ' +
  'The user always understands what is going on; that principle outranks speed.';
