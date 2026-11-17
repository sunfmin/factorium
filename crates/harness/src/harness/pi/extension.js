// factorium's extension for pi: what the tool adds to a pi
// session that pi takes on no flag. The tool carries this file, keeps it in
// its own user directory for pi, and names it on the line that
// starts the session; nothing else loads it.
//
//   - The guard is asked before every tool call, with the
//     call as pi describes it. The guard, not this file, decides. A call the guard did not allow is
//     refused, with what the guard said; so is one it could not be asked of.
//   - The rules of the worktree, and the added rules, are loaded as pi
//     loads a rules file. pi finds the start directory's own, in the directory it starts
//     in; the worktree and the added directory are others.
//
// It is plain JavaScript and imports nothing of pi's: only what Node has.
import { spawnSync } from "node:child_process"
import { existsSync, readFileSync } from "node:fs"
import { join } from "node:path"

// The variables the start puts these in (`factorium_harness::harness::pi`).
const GUARD_ENV = "FACTORIUM_GUARD"
const WORKTREE_ENV = "FACTORIUM_WORKTREE"
const ADDED_ENV = "FACTORIUM_ADDED_DIR"
// Where a directory keeps its rules: the first of these it has, as pi reads
// the directory it starts in.
const RULES = ["AGENTS.md", "CLAUDE.md"]
// Where the added directory keeps its rules (`factorium_harness::start::RULES`).
const ADDED_RULES = ["CLAUDE.md"]
// How long the guard is given to answer one call, in milliseconds.
const GUARD_TIMEOUT = 60000

function fact(name) {
  const value = process.env[name]
  if (!value) throw new Error(`factorium: ${name} is not set: nothing is allowed unread`)
  return value
}

export default function (pi) {
  // The guard answers with its exit status: 0 allows the call, and anything
  // else — a refusal, a guard that is not there, one that did not answer in
  // time — refuses it. A handler that throws refuses the call too.
  pi.on("tool_call", (event, ctx) => {
    const described = JSON.stringify({ toolName: event.toolName, input: event.input ?? {} })
    const asked = spawnSync("/bin/sh", ["-c", fact(GUARD_ENV)], {
      input: described,
      cwd: ctx.cwd,
      encoding: "utf8",
      timeout: GUARD_TIMEOUT,
    })
    if (!asked.error && asked.status === 0) return undefined
    const said = (asked.stderr ?? "").trim()
    const failed = asked.error ? `: ${asked.error.message}` : ""
    return { block: true, reason: said || `factorium guard: it could not be asked${failed}` }
  })

  pi.on("before_agent_start", (event) => {
    const loaded = event.systemPromptOptions.contextFiles
    const kept = [
      [fact(WORKTREE_ENV), RULES],
      [fact(ADDED_ENV), ADDED_RULES],
    ]
    for (const [dir, names] of kept) {
      for (const name of names) {
        const path = join(dir, name)
        if (!existsSync(path)) continue
        if (!loaded.some((file) => file.path === path)) {
          loaded.push({ path, content: readFileSync(path, "utf8") })
        }
        break
      }
    }
  })
}
