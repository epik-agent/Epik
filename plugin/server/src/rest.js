// GitHub REST calls, which work where GraphQL is blocked.

import { gh } from './gh.js';

// The JSON at a REST `path`.
export const get = (path) => gh(['api', path], { json: true });

// Every item of the paginated REST list at `path`. Endpoints that wrap their
// list in an object name its key as `field`.
export async function list(path, field) {
  const separator = path.includes('?') ? '&' : '?';
  const pages = await gh(['api', '--paginate', '--slurp', `${path}${separator}per_page=100`], { json: true });
  return pages.flatMap((page) => (field ? page[field] : page));
}
