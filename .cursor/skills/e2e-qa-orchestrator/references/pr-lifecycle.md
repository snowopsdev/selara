# PR review and merge

Use this workflow for every campaign PR. The coordinator owns the PR registry, review decisions, dependency order, and merges. Workers can implement fixes and collect evidence.

Delivery has three values. `local` keeps commits and drafts locally. `review` publishes and completes review without merging. `merge` continues through landing on the user's target branch, normally `main`, and validation there. Record the user's authorization once. A request for autonomous delivery through main includes review requests, replies, thread resolution, and merges that pass this workflow. Do not ask again at each PR.

## Confirm prerequisites

Inspect repository instructions, the target branch, push and merge access, branch rules, required reviews, status checks, merge methods, queue requirements, and installed stack tools. Confirm Codex code review is available for the repository. Marking a PR ready does not install or enable the integration. Use configured access; report missing setup without changing account settings or weakening branch rules. [Codex review setup](https://learn.chatgpt.com/docs/third-party/github)

Record the PR URL, number, head branch and SHA, base branch and SHA, owner, finding IDs, dependencies, delivery mode, review requests, evidence paths, and next action. Keep check-run IDs and thread IDs so resumption can distinguish new feedback from handled feedback. Include every PR the campaign publishes, including a required reporting PR.

## Open and mark ready

1. Group commits by a fix or a dependent set of fixes. Describe the failure, cause, change, tests, and remaining risks. Follow repository commit and PR conventions.
2. Open a draft if work is still in progress. Once local regression, affected journeys, lint, and type checks pass, mark it ready. If it is already ready, leave it ready. Do not wait for Codex while the PR is still a draft.
3. Record the transition and inspect the review activity. With automatic review configured, moving from draft to ready starts Codex review. [OpenAI's description of the trigger](https://openai.com/index/introducing-upgrades-to-codex/)

With resolved repository and PR values, the CLI operation is:

```bash
gh pr ready "$PR_NUMBER" --repo "$REPOSITORY"
```

Ready means reviewers can begin. It does not mean the PR can merge. Continue inspecting CI, review requests, reviews, top-level comments, and inline review threads.

## Confirm Codex reviewed the current head

Wait for the configured automatic review. If no review starts within the recorded startup window, check existing requests and post one `@codex review` comment for that head. Record the comment ID, request time, and head SHA so polling does not send duplicates. Confirm a completed review response. A start reaction or an empty comment list is not completion. [Requesting Codex review](https://learn.chatgpt.com/docs/third-party/github)

Tie the result to the current head using the review's commit metadata. If the integration returns a comment or clean-result reaction instead, retain the associated request and verify that the head did not change during that review. If the evidence cannot establish what was reviewed, request a fresh review. Do not invent a required approval from a bot that only submits comments.

After pushing a fix or restacking, check whether that head already has a review. If it does not, request one and wait for the result. Do not assume automatic review runs after every push. Keep required human approvals separate from Codex review.

## Address comments and resolve conversations

Read all pages of reviews, PR comments, and review threads. Track each new comment or thread by ID. Do not use only an aggregate review summary.

For each actionable finding:

1. Check the claim against the current code and reproduction. Give related findings one repair owner.
2. Fix valid findings, add or update a regression test, run the relevant checks, and push the change.
3. Reply in the original conversation with what changed, the commit, and the validation result. Use one useful reply per completed response, not a comment on every poll.
4. Resolve that specific review thread only after the fix addresses the concern and the evidence supports it. Use its exact thread ID through the supported GitHub UI or API, then confirm `isResolved`. Top-level PR comments have no review-thread resolution flag; record their disposition instead.
5. If a finding is incorrect, explain the code behavior and evidence respectfully. Resolve it only when the concern is demonstrably answered and repository practice permits author resolution. Leave a disputed or explicit changes-requested concern open for the reviewer. Resolving a thread does not dismiss a required review.

Do not resolve threads in bulk or assume an outdated diff means the concern is fixed. Recheck resolved concerns after later edits. Separate optional suggestions from defects, but record their disposition. A requested out-of-scope change may need a follow-up issue; a blocking concern must still be resolved before merge.

## Keep supervising

Continue the loop: read updates, fix findings, push, rerun validation, obtain review of the new head, reply, resolve addressed threads, and inspect merge eligibility.

Use bounded waits, normally 30 to 60 seconds initially, and back off within the platform's limits. Save the next check time and any service retry interval. Continue independent PRs while one waits. Report meaningful changes and actual blockers without flooding the PR with polling comments.

Queued checks and an active reviewer are wait states. Service failure, exhausted review quota, missing access, or a required human decision can become external blockers after the documented recovery attempts. Preserve exact next actions. Do not mark such PRs complete or resolve their blockers to make the status green.

A discovery-pass limit does not stop supervision of already-created PRs. Honor an explicit overall deadline or stop request. If the platform interrupts the run, save a resumable checkpoint. Claim background supervision only when an actual scheduler or continuing agent is running.

## Merge gate

Before each merge or queue submission, refresh GitHub state and local evidence for the current head and base. Require all of the following:

- The PR is open, ready, in scope, and targets the intended branch or recorded stack base.
- Local regression, affected journeys, integration checks, lint, type checks, build, and other applicable repository validations pass for the current change.
- Every required GitHub check and every applicable campaign validation has finished successfully. Pending, failed, cancelled, timed-out, missing, or unknown results block merge. A skipped or neutral result is not a passing test. Accept it only when the check is a documented non-test or conditional job whose condition is false, and all applicable tests still pass. Record that classification.
- Codex finished reviewing the current head. Its actionable findings have been addressed and verified.
- Required human and code-owner approvals are current, no blocking changes-requested review remains, and review conversations are resolved after their concerns were addressed.
- The diff contains only intended changes. Secrets, debug code, and weakened checks are absent.
- Dependencies are included or have landed, the branch is mergeable, and base freshness or queue requirements are satisfied.

Record the gate's head SHA, base SHA, check time, check runs, review evidence, thread evidence, and validation evidence. Any new commit, base change, review, or reopened thread invalidates the affected evidence. Refresh it before proceeding.

Use a server-side expected-head condition where supported. For individual PRs, `gh pr merge --match-head-commit` rejects a changed head. Select the repository's allowed merge method and use the merge queue when required. Never use admin bypass. Enabling auto-merge is not proof of merging; do not arm it before this skill's extra review and validation conditions pass. If those conditions cannot be enforced while queued, keep supervising or wait for a controlled merge. [GitHub CLI merge options](https://cli.github.com/manual/gh_pr_merge)

GitHub's server-side branch and queue rules remain the final check. Do not disable them, dismiss required approvals, or weaken tests to land a PR.

## Use a stack when it fits

Use a stack for real dependencies that benefit from separate review. Keep independent fixes separate. Record a dependency graph and check the installed tool's support before creating a native stack.

For a supported native GitHub stack, merge only an approved prefix containing campaign PRs. Require current review and validation for every included PR and test the combined result. Native stack merging can land a contiguous group from the bottom through the selected PR; do not accidentally include unrelated ancestors. Stack auto-merge is currently unsupported, while merge queues support stack order. Recheck these capabilities when running. [GitHub stack merge behavior](https://docs.github.com/en/pull-requests/how-tos/merge-and-close-pull-requests/merging-stacked-pull-requests)

When native stack merging is unavailable, use linked dependent PRs and merge from the bottom up. After a parent lands, update the child onto main with the repository's normal tooling. Verify its diff contains only the intended remaining changes, especially after a squash merge. Rerun affected validation and review for changed heads or bases before merging the child. Never count a child merged only into an unlanded feature branch as delivered to main.

Restacking may rewrite campaign branch history. Preserve old SHAs first and use supported stack tooling. If a manual remote update is necessary, restrict it to an exclusively owned campaign branch and use an explicit expected remote SHA with a lease. Abort on a concurrent change. Never force-push main, another contributor's work, or use an unrestricted force update.

After a stack or queue changes commits, fetch the new state, rerun the merge gate, and check the combined integration result. A removed queue entry may require dependent entries to be reevaluated. Queue acceptance is not landing.

## Verify landing

Wait for GitHub to confirm each PR merged. Record its merge commit and time. Verify the change reached main, accounting for squash or rebase merges rather than assuming the old head must be an ancestor.

Watch checks that run on the landed commit and run the applicable smoke journey against main. If validation fails, keep the campaign open, record the regression, and prepare a fix or follow the repository's authorized rollback procedure. Stop dependent merges until the result is understood.

The delivery is complete when every campaign PR has landed and its checks on main pass, or an explicitly superseded PR points to a replacement that landed. Save the report and any reporting PR's final merge receipt locally. Do not open another reporting PR solely to record that receipt.

## Recorded evidence is not GitHub enforcement

`report.py` validates stored receipts and prevents premature campaign completion. It does not merge PRs, poll GitHub, enforce branch protection, or prove that an evidence path is truthful. The coordinator must perform this workflow and inspect live results. A standalone monitoring service would be separate software.
