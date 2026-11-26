# 6. factorium has a fixed set of concepts, listed in the glossary

Status: accepted (the owner approved factorium's specification on
2026-11-17, its concepts among it)

## Context

factorium runs agents while nobody watches, and every concept it has is
one more thing the watch, a worker and the operator must read the same
way: in doctor's findings, in each command's `--help`, in the database.
A concept added in passing, to fit one change, is a word only that change
knows, and the next reader guesses at it.

## Decision

- factorium has the concepts `GLOSSARY.md` lists under "The concepts",
  and no others. Adding one needs the owner's approval and an entry
  there, in the same change.
- A word used inside a concept (a role, a hold, an episode) is listed
  under "Words within them": it adds no concept.
- doctor is not a concept of its own: it is the findings computed from
  the database and the host's readings, each with how to fix it.

## Consequences

- A change that needs a new concept is escalated to the owner, not
  written.
- Each concept's mechanism is said once: how it works by its command's
  `--help` and its code, why by its ADR, which the glossary names.
