# Run 5: can a cheap model write Plumb? (DeepSeek V4 Flash, 2026-10-02)

One task from run 4 (`14-notes-api`: HTTP JSON API on SQLite, 17 hidden
tests), the run 4 writer prompt, the run 4 harness (`bin/`). The agent is
[pi](https://github.com/earendil-works/pi) 0.84 in print mode
(`bin/run-pi`), model `deepseek/deepseek-v4-flash` through OpenRouter,
pinned to DeepInfra (`pi-agent/models.json`). The hidden tests and
reference solutions were removed from this copy of `tasks/`; the checks run
from run 4's directory.

## Result

| | hidden tests, first compiled | final | builds (failed) | lines | time | cost |
|---|---|---|---|---|---|---|
| Plumb, DeepSeek V4 Flash | 14/17 | 16/17 | 11 (4) | 158 | 23.5 min | $0.064 |
| Plumb, Haiku 4.5 (run 4) | 13/17 | 13/17 | 6 (2) | 176 | | |
| Plumb, Sonnet 5.5 (run 4) | 17/17 | 17/17 | 5 (3) | 203 | 68 s | |

46 model requests, 2.4M input tokens (mostly cached), 35K output.

The one final failure: `POST` with a missing or wrongly typed field returns
`invalid JSON` instead of `invalid field: <name>`. The program decodes the
body with `json.decode<NoteInput>`, whose error is only a message
(`json: at $.title: ...`), so it can't tell which field failed; its PATCH
handler walks `json.parse` and gets it right.

## Plumb errors it hit (each fixed from the compiler message)

- `if x is not json.Value.Object(fields)`: `is not` with a binding pattern
  (also in the discarded attempt; it expects this form).
- `return` inside a lambda (also in the discarded attempt).
- `title.length()`: `length` is a field.
- a `Connection` must be opened with `with`.
- a call with 3+ arguments needs names after the first.
- a build before `main.plumb` existed.

## What went wrong around it (not counted)

- The cheapest OpenRouter provider for this model (OpenInference) garbled
  tool results: the model kept re-reading the spec ("the output seems
  garbled") and, looking around, opened the hidden `check.py` and `ref.py`.
  Pinning DeepInfra fixed it. Those attempts are in `results/attempt*`.
- That looping agent wasn't stopped (`pkill -f` missed it: pi renames its
  process to `pi`) and ran alongside the next attempt in the same directory
  (`results/attempt2-mixed`, discarded).
- In the counted run the agent's own test script hung in `wait` on its
  background server; the server was killed once by hand
  (`results/interventions.md`). pi's bash tool has no timeout.

Total OpenRouter spend for the whole session, all attempts: $0.18.
