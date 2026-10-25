// Runs as `gh` under fakeGithub. See fake-github.js.

import { appendFileSync } from 'node:fs';

const args = process.argv.slice(2);
appendFileSync(process.env.FAKE_GITHUB_LOG, `${JSON.stringify(args)}\n`);
const call = args.join(' ');
const reply = JSON.parse(process.env.FAKE_GITHUB_REPLIES).find(([needle]) => call.includes(needle));
if (reply) {
  console.log(JSON.stringify(reply[1]));
} else {
  console.error(`fake gh has no reply for: gh ${call}`);
  process.exit(1);
}
