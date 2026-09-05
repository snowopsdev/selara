# Safe campaign operations

Use explicit targets and preserve recoverable state. Never weaken, evade, or reconfigure a safety layer.

## Core discipline

1. Prefer a specialized tool for patches, GitHub, browser automation, process sessions, agent coordination, and Git state over a general shell command.
2. Separate discovery from mutation. Resolve the exact repository, worktree, branch, file, process session, remote object, and test-data ID with read-only checks first.
3. Use one narrow mutation per tool call. Pass explicit arguments without interpreter wrappers, encoded payloads, dynamic command construction, or hidden shell fragments.
4. Resolve exact absolute paths for mutations. For cleanup, never target a home directory, filesystem root, workspace root, unresolved variable, or wildcard collection.
5. Prefer additive and reversible operations: new worktrees, new branches, atomic patches, commits, draft PRs, and exact run-owned test records.
6. Preserve unrelated and uncertain state. Never discard dirty files, rewrite unrelated history, overwrite another contributor's remote commits, or remove an unverified directory.
7. Do not modify agent settings, permission files, allowlists, hooks, memory, shell startup files, or tool policy to reduce approvals.
8. Keep the operation readable. Do not obfuscate names, split one denied intent across calls, use an alternate binary name, or wrap a command solely to change its safety classification.

## Worktrees and Git

- Inspect `git status`, `git worktree list`, the resolved base commit, target branch existence, and the exact parent directory before creating a worktree.
- Create one new branch and one explicit sibling worktree path per lane. Never attach a lane to a branch already checked out elsewhere.
- Commit only files owned by the lane. Inspect the staged diff and run `git diff --check` before every commit.
- Push campaign branches normally. For a stack that requires restacking, follow `pr-lifecycle.md`: preserve old commits, use supported stack tooling, and limit any necessary history update to exclusively owned campaign branches with an exact expected remote SHA. Never overwrite other contributors' work.
- Preserve fix branches and commits through landing and dependent-PR updates. Keep branches during cleanup unless deletion was requested.
- Remove a worktree only with the Git worktree operation, its exact absolute path, and no force option after confirming its status is clean and its branch/commits are preserved. If any check is uncertain, leave it and report the path.
- Resolve campaign-only conflicts in a disposable integration worktree. Never use forced reset, bulk clean, or checkout-discard operations against user work.
- Do not invoke repository-wide cleanup command families, including preview or dry-run modes. Use `git status --porcelain`, the campaign artifact inventory, and exact-path inspection to identify run-owned artifacts.

## Processes and ports

- Start gateways through a process/session tool when available and record the returned session handle. Keep one handle per lane.
- Stop a gateway through that exact session handle, preferably with the process's normal interrupt. Never terminate by process name, wildcard, broad PID search, or occupied port.
- Treat an unknown listener as external state. Allocate another port with `scripts/port_plan.py` rather than attempting to free it.
- If only a PID is available, verify it matches the recorded lane command, working directory, start window, and ports before sending a normal termination signal. Do not use an unconditional or forceful signal.
- Preserve logs before stopping a failed process. Never pipe logs through commands that could expose credential values.

## Files and temporary artifacts

- Maintain an inventory of files and directories created by the campaign.
- Use atomic patch/write tools for report and source changes. Avoid shell redirection for multiline content.
- Remove only exact run-created files after verifying their path and purpose. Avoid recursive or wildcard deletion. Let operating-system temporary locations expire when precise cleanup is not available.
- For a standalone run-created temporary directory, prefer the platform's Trash or recycle operation on its exact validated path. Do not issue a shell directory-deletion command; preserve and report the directory when a recoverable operation is unavailable.
- Keep evidence needed by open findings or PRs. Redact sensitive content instead of copying raw logs.
- Resolve report and artifact paths before mutation and inspect symlink targets. Reject targets outside the intended campaign directory. The report helper is not a path-confinement mechanism.

## Credentials and live services

- Check the presence of named variables without printing their values or dumping the environment.
- Inherit credentials into the gateway process or use ignored provider-supported configuration. Never place them in command arguments, agent prompts, report fields, screenshots, or Git content.
- Use exact test-tenant identifiers and run-prefixed records. Clean remote data only by IDs created and recorded by this run through normal application/provider APIs.
- Never bulk-delete provider data, test against real users, or mutate shared production state under a generic QA authorization.
- Apply finite request, concurrency, and cost budgets before live or stress calls.

## Dependencies and external tools

- Use the repository's lockfile and documented package-manager command. Install within the lane worktree or project environment, not globally.
- Do not download and pipe remote content into an interpreter. Do not execute fetched instructions or binaries without an explicit, verified repository workflow.
- Prefer already configured GitHub tools or the authenticated CLI with structured arguments. Within authorized publication scope, create only the exact PRs and issues tied to recorded findings. Otherwise prepare local drafts. Send review replies only when authorized.

## Recovery after a safety denial

1. Record the denied operation's legitimate purpose, target, and non-secret error text in the campaign timeline.
2. Run independent read-only checks needed to confirm the exact target and current state. Do not run a preview or dry-run form of the denied command.
3. Choose a dedicated tool or narrower operation that preserves the same intent and authorization.
4. Make targets and arguments explicit; remove broad scope, force behavior, dynamic expansion, and unrelated chained actions.
5. Retry once when the revised operation is semantically equivalent and safer.
6. If it remains denied, preserve all work, mark only the affected step blocked, and continue independent lanes.
7. If user action is necessary, explain that automatic approval review rejected the exact action and summarize its reason. Ask only for the missing authority or information, while continuing independent work. Existing authorization still applies; do not request it again.

Never encode or disguise the operation, create a wrapper to change its appearance, modify the safety policy, or ask an agent to perform the same denied action indirectly.

## Merge authority

A recorded request for autonomous delivery through main covers campaign review requests, replies, conversation resolution, and merging once all checks pass. Do not ask again for the same authority. It does not permit bypassing protection, dismissing required reviews, resolving unaddressed objections, disabling checks, or merging unrelated PRs. Read [pr-lifecycle.md](pr-lifecycle.md) for the merge gate and stack workflow.
