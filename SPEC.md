## 9. Decisions, and why

1. **Repo declarations live in the repo** (`factorium.toml`): they change
   with the code and are reviewed with it.
2. **Every checkout has a proving branch**: one path. A repo with no slow
   steps promotes as soon as it lands.
3. **A landing red twice in a row goes to the watch**; no second worker in
   a new worktree. A merge red on a proving tip that moved since the last
   red's tip, which the branch holds, is not the second: the worker did
   what the first asked, and in one day eight watch rounds only said
   "merge the new tip" (factorium#171). `moved_reds_max` bounds how many
   in a row are let off.
4. **No Flux.** Scenarios and mutation hold the rules; afkconvoy's fast gate
   spent 65 of its 138 seconds in Flux.
5. **factorium is the one writer of a managed repo's base.** A repo
   factorium manages has its base moved only by promotion; anything else
   that moves the base turns promotion red.
6. **A landing waits for no review** (factorium#167). It lands on the fast
   gate alone and reaches only proving, which is promoted after the slow
   gate; verdicts and rework stay out of factorium (§7). Review comes after
   landing, from outside: Guild's warden, and control's job on a proving
   branch, file what they find as new issues. What would reopen it: a
   landing that harms before review can catch it (secrets, data loss,
   something externally visible) — then a finding that holds promotion
   (`factorium hold`), not an approval on every landing; or measured
   evidence that findings after landing cost more than review before it.
