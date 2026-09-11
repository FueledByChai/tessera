Interview the owner of a new project on the first day, and leave behind what the loop needs
before any code exists: the Project rules, the decision records, the first epics, and a check
that runs. Interrogate first, write second. Follow the standing instructions in `AGENTS.md`
(the loop section; the Project rules are what this session writes).

Settings come from `.loop.toml` when it exists (`scripts/loop-config.sh --all`); this session
fills it in. The kit's templates are under `loop/templates/` after `install.sh`: the decision
record, and a check skeleton per stack under `loop/templates/check/`.

## 1. Ground yourself

Read `AGENTS.md`, `.loop.toml`, and the decision index (`docs/decisions/README.md` when it
exists), so a project that is not quite new is not asked what it already knows. Look at what
is in the repository already: a manifest (`Cargo.toml`, `pyproject.toml`, `package.json`,
`pom.xml`, `build.gradle`, `go.mod`), a README, any code. Never ask what a file already answers.

## 2. Interview

Ask in rounds, one area per round, at most four questions each, every question with a
recommended default first, in whatever way the harness offers for asking the owner. Each area
ends in one of two things: a decision record written in this session
(`scripts/decisions.sh new "<title>"`, then its Context, Decision, Alternatives, Consequences,
and what would show it was wrong), or a dated deferral: a record whose Status line reads
`deferred until YYYY-MM-DD` and whose Decision section says what has to be learned first. Do
not draft anything until every area has one or the other. Push back on "whatever is easiest"
with a concrete option to accept or reject, and restate the owner's answers in your own words
at the start of the next round.

The five areas, in order:

1. **Who and where.** Who uses it, how many of them, from where (a terminal, a browser, a
   phone, another program), how often, and what they do today instead. This decides the
   shape: a CLI, a service, a web app, a library.
2. **The data.** What it stores, how much, how fast it grows, how long it is kept, whether
   one person or many write it, what must never be lost, and what must never leave the
   machine. This decides whether there is a database, which kind, and where it lives.
3. **Runtime and deploy.** The language and its version, the machine it runs on, how it gets
   there (a script, a container, a host's deploy), how it is restarted, and what is already
   installed. This decides the stack row below and whether the project gets a deploy loop.
4. **The UI.** None, a terminal, or a browser; who has to be able to read it; the look the
   owner wants and any conventions (density, colour, what must fit on one screen). This
   becomes the UI conventions in the Project rules and the first fixture the checks render.
5. **Non-negotiables.** What must never be touched, committed, sent, or deleted; what must be
   private; what must be exact; what would make the owner stop trusting the tool. These
   become the rules the prompts refuse to cross and the review paths the loop labels for a
   human.

## 3. Write

Show the owner everything below in chat and get a yes before writing.

- **Decision records.** One per decision from the interview, numbered from where the index
  left off; deferrals as records with `Status: deferred until YYYY-MM-DD`.
- **`AGENTS.md`, the Project rules section.** Replace the placeholder with: what the project
  is, in two sentences; Layout (where code, tests, docs, and data live, and what is private);
  Build, run, and restart (the exact commands); The check (what `scripts/check.sh` covers and
  how long it takes); the rules from area 5 as bullets that begin with "Never" or "Always";
  UI conventions from area 4 when there is a UI; Docs to keep current. Cite the record numbers
  where a rule comes from a decision.
- **The product backlog**, `docs/PRODUCT_BACKLOG.md` unless the owner names another path:
  the format grill-me expects (an epic per area of the product the interview surfaced, each
  with one or two stories in the `### BT-nnn — <title>` shape with Status, User story, and
  Acceptance criteria). Two to four epics; the stories are the ones the owner would build
  first, not a roadmap.
- **The ticket file** (`backlog` in `.loop.toml`, `BACKLOG.md` by default): the protocol
  header grill-me's tickets assume, one `## <section>` per epic, and a first ticket whose
  done line is "`scripts/check.sh` passes in CI on this repository's first pull request".
- **`.loop.toml`** from `loop.toml.example`: `default_branch`, `backlog`, `check` and
  `check_fast`, `code_paths` and `proof_paths` and `proof_pattern` for the stack from the
  table below, `coverage` when the stack row gives a command, `review_paths` from area 5,
  `kit` and `kit_ref` (the kit's URL and its current tag).
- **`scripts/check.sh`**, copied from the stack's skeleton under `loop/templates/check/` and
  made executable. The skeleton runs the loop's own checks first, then the stack's format,
  lint, test, and build steps, each skipped with a note while the manifest is absent, so it
  exits 0 on an empty repository and starts failing as code arrives. Fill the TODO lines the
  interview answered; leave the rest as TODO with the question they wait on.

The stack table. A stack not in it gets `other.sh`, whose steps are all TODO.

| Stack | Skeleton | `code_paths` | `proof_paths` | `proof_pattern` |
| --- | --- | --- | --- | --- |
| Rust | `rust.sh` | `["src/"]` | `["tests/"]` | `#\\[test\\]` |
| Python | `python.sh` | `["src/"]` | `["tests/"]` | `def test_` |
| Node or TypeScript | `node.sh` | `["src/"]` | `["test/", "tests/", "__tests__/"]` | `\\b(test|it)\\(` |
| Java, Maven or Gradle | `java.sh` | `["src/main/"]` | `["src/test/"]` | `@Test` |
| Go | `go.sh` | `["./"]` | `["_test.go"]` | `func Test` |

Then run `scripts/check.sh` and show its output: it must exit 0 before you finish. Run
`scripts/decisions.sh --check` and `scripts/prompt-check.sh` as well.

## 4. Hand off

Commit everything as one commit, subject `Project: first-day interview`, with the trailer the
config requires. On a repository with no pull request rules yet this commit sits on the
default branch and the owner pushes it; from the next change on, the loop's rules apply
(`scripts/open-ticket-pr.sh`, never push the default branch). Then report: the records written
and deferred (with their dates), the epics and the first ticket, the stack and the skeleton
used, what the check covers today and which TODO lines remain, and the two things the owner
does next: apply the repository settings and ruleset from the kit README, and run
`/next-ticket`.

Rules: this prompt writes only the files named above. It never invents a decision the owner
did not make: an area without an answer is a dated deferral, not a guess. Nothing private the
owner names in area 5 goes into the repository.
