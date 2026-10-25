// References to issues and pull requests. A number names an issue only within
// its repository, so a reference is its URL, which names both. GitHub names
// are case-insensitive, so a URL's repository is lowercased: two spellings of
// one issue give one URL, and URLs compare as strings.

// The URL of issue `number` in `repo`.
export const issueUrl = (repo, number) => `https://github.com/${repo.toLowerCase()}/issues/${number}`;

// The URL of pull request `number` in `repo`.
export const pullUrl = (repo, number) => `https://github.com/${repo.toLowerCase()}/pull/${number}`;

// The `owner/name` of the repository an issue or pull request from the REST
// API is in.
export const repoOf = (item) => item.repository_url.split('/repos/')[1];

// Whether `a` and `b`, each `owner/name`, are one repository.
export const sameRepo = (a, b) => a.toLowerCase() === b.toLowerCase();

// Orders URLs by repository, then numerically by number.
export const byUrl = (a, b) => a.localeCompare(b, 'en', { numeric: true });
