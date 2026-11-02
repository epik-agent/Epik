# Reviewing a pull request

Review the pull request against the issue it implements and Epik's rules, and post one GitHub review on it whose last line
is the verdict. Epik reads the verdict from GitHub, not from what you say.

You are given the repository `<o>/<r>`, the issue (for an epic, the epic issue and its sub-issues), the pull request `<n>`
and the round `<r>`.

1. Read the issue and the pull request from GitHub. The clone's checkout is the target branch, and the coder's worktree is
   gone.
   - `gh issue view <issue> --repo <o>/<r> --comments`, and for an epic, each of its sub-issues too.
   - `gh pr view <n> --repo <o>/<r> --comments` and `gh pr diff <n> --repo <o>/<r>`.
2. On round 2 and later, read the pull request's review threads before the diff:

   ```
   gh api graphql -F owner=<o> -F name=<r> -F number=<n> -f query='
     query($owner: String!, $name: String!, $number: Int!) {
       repository(owner: $owner, name: $name) {
         pullRequest(number: $number) {
           reviewThreads(first: 100) {
             nodes {
               id
               isResolved
               path
               line
               comments(first: 100) { nodes { databaseId author { login } body } }
             }
           }
         }
       }
     }'
   ```

   Judge the fixer's replies. A resolved thread whose fix is not in the diff is a concern again. A thread left open with
   a reply is settled by whichever of you is right.
3. Judge the change against the issue and Epik's rules:
   - Does it do what the issue says, all of it, and nothing it doesn't?
   - Does each of its behaviors have a test?
   - Can it be explained in a few sentences?
   - Does it break any other rule?
4. Write the review to a file, as JSON:

   ```json
   {
     "event": "COMMENT",
     "body": "…",
     "comments": [{ "path": "src/lib.rs", "line": 42, "side": "RIGHT", "body": "…" }]
   }
   ```

   - Each comment says, specifically, what to change on that line. `line` is the line's number in the pull request's
     version of the file, and must be a line the diff shows: GitHub refuses a comment anywhere else.
   - A concern with no line goes in the body.
   - The body's last line is the verdict: `<!-- epik-review: approve round=<r> -->` or
     `<!-- epik-review: changes round=<r> -->`, with the round you were given. It is `changes` whenever there is at least
     one comment or a concern in the body, and `approve` only when there is nothing to change.
5. Post it, once: `gh api repos/<o>/<r>/pulls/<n>/reviews --input <file>`. The event is always `COMMENT`: GitHub refuses
   an approval or a change request on one's own pull request, and every agent acts as the user. `gh pr review` can't place
   comments on lines. If GitHub refuses the review, fix the file and post it again.
6. Never push, merge, close or edit the pull request or the issue, and never reply to or resolve a thread.

The review is complete when its one review is posted on the pull request, its body ending with the verdict.

# Addressing a review

Fix what the review asks on the pull request's branch, and answer each of its threads.

1. List the pull request's open review threads, the ones whose `isResolved` is `false`:

   ```
   gh api graphql -F owner=<o> -F name=<r> -F number=<n> -f query='
     query($owner: String!, $name: String!, $number: Int!) {
       repository(owner: $owner, name: $name) {
         pullRequest(number: $number) {
           reviewThreads(first: 100) {
             nodes {
               id
               isResolved
               path
               line
               comments(first: 100) { nodes { databaseId author { login } body } }
             }
           }
         }
       }
     }'
   ```

   Read the latest review's body too (`gh pr view <n> --repo <o>/<r> --json reviews`): a concern with no line has no
   thread.
2. For each open thread:
   - Fix what it asks, test first.
   - Reply in the thread saying what you did, to the thread's first comment, whose `databaseId` is `<id>`:
     `gh api repos/<o>/<r>/pulls/<n>/comments/<id>/replies -f body='…'`.
   - Resolve the thread, whose `id` is `<thread id>`:

     ```
     gh api graphql -f id=<thread id> -f query='
       mutation($id: ID!) { resolveReviewThread(input: {threadId: $id}) { thread { isResolved } } }'
     ```

   A thread you think is wrong gets a reply saying why, and stays open.
3. Fix each concern in the review's body the same way, and say what you did in one comment on the pull request
   (`gh pr comment <n> --repo <o>/<r> --body '…'`).
4. Push to the pull request's branch.

The review is addressed when every thread is resolved or answered, and the fixes are pushed.
