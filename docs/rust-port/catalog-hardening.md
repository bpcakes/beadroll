<!-- Port catalogue for docs/RUST-PORT.md. Generated on 2026-10-09 by reading the Python tests at commit 9867715.
     Each row is a starting classification: confirm it when the test is ported, and correct it here if it is wrong. -->

# tests/test_hardening.py: port catalog

Source: `tests/test_hardening.py` (6,517 lines, 102 classes, 207 test methods; subTests not counted).
Gxx classes are review findings (docs/FINDINGS.md); Vx classes are verified properties. "t1, t2…" = test methods in file order.

**Review 9 corrections.** The 14 `A†` tests pass against Python today, because Python has the crash hooks. Nine of the twelve `B` tests here are unit tests of the git and store crates, not of core: G25 t3, G32, G57 t2, `V1_RunGitTimeoutHygiene`, the four V7 tests and U06 (added in Phase 1) land with Phase 2a (`docs/RUST-PORT.md` §6). Rows that cover several tests ("t1", "t2") are split per method when the inventory is seeded.

**Mechanism labels.** `cli` = runs bead as a subprocess (support.Machine) and inspects exit/stdout/stderr/files/refs (plain git setup, hand-edited store files and deleted remote objects count as cli). `env-fake` = fake ssh/gpg on PATH or via GIT_SSH_COMMAND, scratch GIT_CONFIG_GLOBAL with special settings, GIT_* env tricks, locales, server hooks in the bare remote, local HTTP server, chmod/read-only, umask, ulimit, closed fds, PTYs/signals, timing bounds. `inproc` = calls package functions. `patched` = support.patched / mock in-process. `driver` = Python wrapper that imports the launcher via runpy (or runs it under another interpreter) and monkeypatches inside. "crash hook" = the CLI's own `BEAD_TEST_CRASH_BEFORE_PUSH` (exit 98) / `BEAD_TEST_CRASH_AFTER_PUSH` (exit 99), implemented only in `sync.coordinated()`.

**Port classes.** `A` black-box, runs unmodified against a Rust binary via BEAD_UNDER_TEST. `A†` = A, provided the Rust binary implements the two BEAD_TEST_CRASH_* env hooks with the same exit codes. `B` in-process logic → Rust unit/integration test against crate APIs. `C` needs an injection seam (named). `D` depends on Python-specific behaviour; the Rust property to keep is in the appendix.

| class | #tests | subject (≤12 words) | mechanism | seams/fakes needed | port class |
|---|---|---|---|---|---|
| G01_TrackedRemoteIsNeverAGitOption | 1 | Option-shaped tracked remote never reaches git as an option | cli | none (marker files; dash-named bare `-coord.git`) | A |
| G02_TrackedConfigCannotAdoptAnotherRepositorysStore | 2 | Foreign derived workspace id refused; tracked remote change needs consent | cli (t1 precondition calls `normalize_remote` + sha256 in-process) | none; precondition = sha256(normalize_remote(origin))[:16], computable in the test | A |
| G03_DamagedMetaMovesCoordinationToOrigin | 3 | Malformed or unreadable meta.json never moves coordination to origin | cli; t2 cli+env-fake (chmod 0 meta.json, skip as root) | none | A |
| G04_TrackedInitKeepsHandSetConfig | 3 | `init --tracked` keeps where/ref/unknown keys, never rewrites a password | cli (reads last line of `events/*/*.jsonl`) | none | A |
| G05_OriginsGitReadsAsRemotes | 3 | scp, alias, insteadOf, IPv6, upper-case, `transport::` origins are one workspace | t1–t2 cli; t3 cli+env-fake | fake ssh via GIT_SSH_COMMAND (runs remote cmd locally, FAKE_SSH_ROOT, `git --exec-path`); scratch GIT_CONFIG_GLOBAL with url.insteadOf | A |
| G06_HttpsTokenInTheUserPosition | 2 | Token in https user part never stored, committed or printed | t1 cli; t2 cli+env-fake (closed localhost port, GIT_TERMINAL_PROMPT=0) | none | A |
| G07_ReceiptSurvivesLaterLocalWrites | 3 | Later local writes stop a retry answering from an old receipt | cli + crash hook | BEAD_TEST_CRASH_AFTER_PUSH (exit 99) | A† |
| G08_StaleReceiptsDroppedBeforeTheCommandGoesThrough | 3 | A failed later command keeps the accepted command's receipt | cli + crash hooks; t2 +env-fake (gated pre-receive hook, BEAD_NET_TIMEOUT=1) | BEAD_TEST_CRASH_AFTER_PUSH/BEFORE_PUSH (99/98); BEAD_NET_TIMEOUT | A† |
| G09_ReceiptLoopVariableShadowsTheInFlightReceipt | 1 | Lost ordinary race after a settled receipt retries, not crashes | inproc+patched (wraps `Link.publish` so machine B publishes first) + crash hook | pre-push contention hook (callback between fetch and the claim push), or a `git` shim on PATH; BEAD_TEST_CRASH_AFTER_PUSH | C (the TypeError itself is Python-only; the property stays) |
| G10_RecognisedRetryReportsAClaimThatMovedOn | 3 | Recognised retry reports the claim as it is now | cli + crash hook | BEAD_TEST_CRASH_AFTER_PUSH | A† |
| G11_ServerHookTextDoesNotDecideThePushOutcome | 2 | Hook text relayed as `remote:` never misclassifies an applied push | t1 cli+env-fake (post-receive prints fake `[rejected]`, `kill -9 $PPID`); t2 inproc+patched+env-fake (patched `time.sleep` between rounds runs machine B; `cmd_update` in-process) | t2: pause hook between publish rounds; in-process command entry (Workspace, parser, cmd_update) | A ×1, C ×1 |
| G12_RetriedEndOfClaimIsPublished | 3 | Retried owner close/delete/update-closed publishes the end of claim | driver (runpy launcher; `run_git` → `os._exit(98)` on first `push`) | crash-before-push hook that also covers `publish_soon` pushes (today's env hook covers only coordinated pushes) | C |
| G13_TrackedConfigIsUtf8WhateverTheLocale | 3 | UTF-8 .beadroll.json read identically under Latin-1 and ASCII locales | cli+env-fake (LC_ALL/LANG en_US.ISO8859-1 / en_US.US-ASCII, PYTHONUTF8=0, PYTHONCOERCECLOCALE=0; python3 encoding probe → skip) | locale env only | D (→A once the python3 probe is dropped): `hardening::g13`, the locales asked of `locale -a`, with a C-locale case |
| G14_KeyRetractionLostToConcurrentTarget | 3 | Key-based dependency retraction survives a concurrent target import | t1–t2 cli; t3 inproc (hand-built v1 events → `Journal`, `replay`, `is_blocked`) | t3: replay API over raw events | A ×2, B ×1 |
| G15_NewKeyCapturesPendingImportedDependency | 2 | Minted or renamed keys never capture a pending imported dependency | cli | none (relies on uuid7 clock order of sequential commands) | A |
| G16_ConcurrentProcessesShareNoFetchDestination | 1 | Concurrent syncs and claims on one store never collide on refs | cli, 7 concurrent processes × 3 rounds | none (impl: per-process fetch ref) | A |
| G17_TranslatedGit | 1 | First sync and claim work under German or French git | cli+env-fake (LANG/LC_ALL de_DE/fr_FR.UTF-8; skip if git untranslated) | locale env | A |
| G18_DefiniteRefusalIsNeitherARaceNorNoAnswer | 2 | Policy and transport refusals reported at once with remote's reason | cli+env-fake (refusing pre-receive; read-only fake ssh; elapsed < 5 s / < 10 s) | BEAD_REMOTE; fake ssh via GIT_SSH_COMMAND; scratch GIT_CONFIG_GLOBAL | A |
| G19_ClientHooksOnPrivatePushes | 4 | User client hooks never run on bead's private pushes | cli+env-fake (init.templateDir, global core.hooksPath, hook inside sync.git, config-based `[hook]` with `git hook list` probe) | scratch GIT_CONFIG_GLOBAL | A |
| G20_TransportSettings | 4 | Checkout and global transport settings reach bead's fetch and push | cli+env-fake (smart-HTTP over `git http-backend` demanding each header exactly once; includeIf gitdir; checkout extraheader; fake `ssh` on PATH; GIT_SSH + GIT_SSH_VARIANT; core.sshCommand) | harness HTTP server; fake `ssh`/`user-ssh` on PATH | A |
| G21_RemoteResolvedAgainstTheCwd | 5 | Relative and named remotes resolve like git from subdirectories | cli | none | A |
| G22_OriginIsNotWhereGitPushes | 3 | pushurl, mirror fetch url and extra urls handled like git | cli | none | A |
| G23_TrackedConfigReadFromWorkingTreeOnly | 2 | Older-branch worktrees and bare clones find the tracked workspace | cli | none | A |
| G24_RenamedRepositoryLosesItsTracker | 2 | Renamed origin keeps its tracker; foreign `--remote` tracker still refused | cli | none | A |
| G25_FirstSyncRemoteIsFrozen | 3 | Origin changes followed; old remembered origin spelling not preferred | t1–t2 cli; t3 inproc (`Workspace(...).remote()`, `save_meta()` under patched BEAD_HOME) | t3: Workspace API | A ×2, B ×1 |
| G26_ObjectFormat | 3 | sync.git hashes like the project (SHA-256 or SHA-1) | cli+env-fake (sha256 repos; init.defaultObjectFormat; sync.git replaced by a sha256 one) | scratch GIT_CONFIG_GLOBAL; git with SHA-256 | A |
| G27_UnfinishedSyncGit | 4 | Empty or object-less sync.git is completed; caller GIT_* env ignored | t2/t4 cli; t1/t3 cli+env-fake (GIT_WORK_TREE, GIT_OBJECT_DIRECTORY at first contact) | scratch GIT_CONFIG_GLOBAL | A |
| G28_OnlyTheLockHolderSweepsPending | 4 | Lockless readers never delete pending attempts or receipts | t1/t4 cli+env-fake (chmod 0 attempt + crash hook); t3 cli+env-fake (`chmod -R a-w` store); t2 inproc+patched (`pending_files` pauses a reader while a subprocess sync runs) | BEAD_TEST_CRASH_AFTER_PUSH; t2: hook between listing `pending/` and reading it | A† ×2, A ×1, C ×1 |
| G29_FailureAfterAcceptanceIsNotNothingHalfDone | 3 | Full volume after remote acceptance isn't reported as IO | driver (`write_atomic` raises ENOSPC for meta.json or for foreign segments) + crash hook | write-failure injection by target (ENOSPC on meta.json; on a foreign segment during pull); BEAD_TEST_CRASH_AFTER_PUSH | C |
| G30_DeepJsonIsBoundedAndScoped | 4 | Deep JSON is bounded: integrity rows, structured errors, import limit | t1 cli (1M-deep lines planted in store); t2 cli + git plumbing (deep manifest.json on remote; deep .beadroll.json); t3 cli; t4 driver (launcher under python3.12/3.13) | none for t1–t3 | A ×3, D ×1 (t4: a mixed-fleet case, `mixed::g30`, Python against Rust each way round) |
| G31_ExportWithUnicodeLineSeparatorsImportsAgain | 2 | Exports with U+2028/U+2029/U+0085 import again; CRLF and blanks accepted | cli | none | A |
| G32_IntegerFieldsStayBeadsReadable | 1 | Writes refuse booleans and out-of-range integers; old events still replay | cli+inproc (`Journal`/`replay`/`read_attempt` guard, then CLI refusals) | replay + read_attempt API | B+A (split) |
| G33_DuplicateIdsInOneImportFailWhole | 2 | Duplicate ids in one import fail the whole import | cli | none | A |
| G34_NonUtf8GitAndPsOutputIsNotATraceback | 5 | Non-UTF-8 branch, origin, origin/HEAD, tree path, parent argv tolerated | t1–t3 cli (packed-refs, config, symbolic-ref with Latin-1 bytes); t4 cli+env-fake (planted remote folder, core.quotePath=false); t5 cli+env-fake (CLAUDE_CODE_SESSION_ID + CODEX_THREAD_ID, Latin-1 parent argv) | process-tree lookup (`ps`) tolerant of non-UTF-8 | A |
| G35_StoreNeverLandsInTheCheckout | 2 | `~` BEAD_HOME expands; relative BEAD_HOME never writes into checkout | cli (BEAD_HOME, HOME env) | none | A |
| G36_HungUpTerminalAndClosedDescriptors | 4 | Hung-up terminal and closed fds: quiet exit, command's own status | cli+env-fake (PTY slave whose master is closed → EIO; `sh -c 'exec … >&-'` / `2>&-`) | none (impl: EPIPE/EIO handling) | A |
| G37_JsonSaysWhenTheEndOfAClaimWasNotPublished | 1 | `--json` close/delete/update mark an unpublished end of claim | cli (bare remote renamed away) | none | A |
| G38_ImportCommentsOfTheWrongShapeAreValidation | 2 | Malformed comment shapes in import are VALIDATION, not crashes | cli | none | A |
| G39_PasswordInATransportUrl | 3 | `https::` transport URL password dropped, never committed or sent | t1–t2 cli (GIT_ASKPASS="", empty global config); t3 cli+env-fake (local HTTP 401→404 recorder) | harness HTTP server | A |
| G40_AtSignAfterTheAuthority | 3 | An '@' after the URL authority is not user-info | cli | none | A |
| G41_DamagedRemoteIsNotAnUnreachableOne | 1 | Remote ref naming lost objects reported as damage, naming the ref | cli (deletes loose commit/tree in bare remote) | none | A |
| G42_UidRetractionMissesTwinOfImportedKey | 2 | Dependency removal or reparent survives merge with twin imported key | cli | none | A |
| G43_RenamedRecordExportsForeignIssueId | 1 | Renamed twin record exports dependencies under its own id | cli | none | A |
| G44_MaterialisedDependencyExportsNullMeta | 2 | Late-materialised dependency exports string created_at and created_by | cli | none | A |
| G45_UnresolvedImportedDependencyInvisible | 1 | show and dep list name an unresolved imported dependency once | cli (JSON and human text) | none | A |
| G46_ImportAcceptsSelfDependency | 1 | Import with a self-dependency fails whole with VALIDATION | cli | none | A |
| G47_StoreIsPrivateByItsOwnModes | 3 | Every store path owner-only; same-second torn fragments both kept | t1–t2 cli+env-fake (umask 022; torn tail + fork scenario; mode scan of BEAD_HOME); t3 inproc+patched (`time.time` fixed; `repair_tail` twice) | t3: clock injection, or fragment names unique under equal timestamps | A ×2, C ×1 |
| G48_FailedAtomicWriteLeavesNothingBehind | 3 | Failed atomic writes leave no temp file; IO names the target | t1 cli (`export -o DIR`); t2–t3 cli+env-fake (`ulimit -f 1` / `-f 16`) | none (impl must ignore SIGXFSZ) | A |
| G49_LocalStateShapesTraceback | 2 | Wrong-shaped meta.json remote or replica.json gives structured errors | cli | none | A |
| G50_MachinesNameKeepsAliasesItCouldNotRead | 3 | `machines --name` never overwrites an aliases.json it cannot read | t1–t2 cli; t3 cli+env-fake (chmod 0, skip as root) | none | A |
| G51_UnusableTrackedConfigIsAnError | 3 | Looping/dangling .beadroll.json link or empty workspace is CONFIG | cli (symlinks) | none | A |
| G52_UnreadableStoreIsIO | 5 | Unlistable/unenterable store or file BEAD_HOME answers IO | t1–t3 cli+env-fake (chmod 0 / 0300 dirs; whole class skipped as root); t4 cli (BEAD_HOME is a regular file); t5 cli+env-fake, skipUnless the *runner's* Python ≥ 3.14 | none | A ×4, D ×1 (t5: `hardening::g52`, run whichever bead and Python, skipped as root like the class) |
| G53_UnreadableIdentityIsIo | 2 | Unreadable replica.json is IO; undecodable stays IDENTITY | t1 cli+env-fake (chmod 0); t2 cli | none | A |
| G54_StaleTwinRuleDropsTheReceipt | 1 | Damaged twin of an accepted release keeps its receipt | inproc+patched (`drop_pending` raises after local confirm) then cli retry | crash hook after local confirm, before pending cleanup | C |
| G55_ReceiptThatIsNotAnObjectBlocksEveryClaim | 1 | Receipt holding non-object JSON is dropped; claims go through | cli | none | A |
| G56_ClosedStdinIsAStructuredError | 1 | `-` readers with closed stdin answer IO/VALIDATION, write nothing | cli+env-fake (`sh -c '… <&-'`) | none (impl must detect closed fd 0 before Rust std reopens it on /dev/null) | A |
| G57_LoneSurrogateInAJournalIsScoped | 2 | Lone surrogates read as U+FFFD, reported; both exports agree | t1 cli (planted foreign segments: escaped `\ud800` and raw ED A0 80); t2 inproc (`read_attempt`) | t2: read_attempt API | A ×1, B ×1 |
| G58_FinishedPushIsNotAnUnansweredOne | 1 | Push git finished isn't "no answer" when helper holds stderr | cli+env-fake (fake ssh spawns `sleep 15` holding git's stderr; BEAD_NET_TIMEOUT=4; must take < 4 s) | BEAD_REMOTE; BEAD_NET_TIMEOUT; fake ssh | A |
| G59_HelpIsPlainText | 3 | Help has no colour codes on a TTY, FORCE_COLOR or `--json` | cli+env-fake (PTY; TERM=xterm-256color; FORCE_COLOR=1; python3 argparse-colour probe → skip) | none | D (→A once the probe is dropped): `hardening::g59` |
| G60_ArabicLetterMarkEscaped | 1 | U+061C escaped as `\u061c` in every human output | cli (human output) | none | A |
| G61_MergedCheckSeesOriginMain | 2 | Merged check uses origin's default branch, slashes included | cli (worktree; scratch global config) | none | A |
| G62_DubiousOwnership | 3 | Dubious-ownership error keeps git's reason and safe.directory fix | cli+env-fake (GIT_TEST_ASSUME_DIFFERENT_OWNER; C/de/fr locales; skip probes) | none | A |
| G63_SettleKeepsLineLevelIntegrityReports | 1 | Invalid-JSON line stays reported when a duplicate forces rebuild | cli (planted foreign segment) | none | A |
| G64_WritesOnABrokenOwnChain | 1 | Writes on a broken own chain: JOURNAL_FORK, nothing appended | cli (edits own segment) | none | A |
| G65_SupersededOwnerCloseSaysSo | 1 | Owner close superseded by a takeover doesn't answer closed | cli | none | A |
| V4_DanglingReplayDeterminism | 2 | Dangling-dependency replay independent of delivery order, 1000 seeds | inproc (splitmix64 generator; `Journal`/`replay`; stats thresholds) | replay API exposing issues (uid, deps, dangling), by_key, key(), pending, integrity | B |
| V4_DanglingOrdersThatHold | 3 | Retraction orders that hold; explicit add wins over key retraction | cli | none | A |
| V3_NonUtf8IdentityAndText | 1 | Non-UTF-8 identity or text in argv/env is exact VALIDATION | cli (bytes argv and env) | none (impl: `args_os`/`vars_os`) | A |
| V3_ImportShapeErrors | 1 | Wrong container or member types in imports are VALIDATION | cli | none | A |
| V3_ForeignFolderDefaultQuotePath | 1 | Non-UTF-8 or U+2028 remote folder skipped under default quotePath | cli+env-fake (git plumbing on remote; empty global config) | none | A |
| V8_PtyOutputMatchesPipeOutput | 1 | PTY output equals pipe output; hostile text always escaped | cli+env-fake (stdout and stderr on two PTYs vs pipes) | none | A |
| V8_SyncUnderAControllingTerminalCannotPrompt | 1 | Sync under a controlling terminal cannot prompt; fails fast | cli+env-fake (controlling PTY via `os.login_tty`; fake ssh opening /dev/tty, first on PATH; BEAD_NET_TIMEOUT=30, must end < 15 s) | none (impl: git in its own session through `setsid`, not merely `process_group(0)`) | A |
| V8_HangupOfTheControllingTerminal | 1 | Closing the controlling terminal ends bead by SIGHUP quietly | cli+env-fake (controlling PTY, SIGHUP reset to default; master closed after 1 KiB) | none | A |
| V8_RemoteMaintenance | 2 | Remote gc/repack after or during operations keeps convergence | cli+env-fake (`gc --prune=now --aggressive` after each op; concurrent repack/prune-packed/`gc --auto` thread; 60 ops on 3 machines) | none | A |
| V8_PruneOnThePrivateSyncRepo | 1 | `gc --prune=now` looping on sync.git causes only transient errors | cli+env-fake (gc loop thread on sync.git; 30 create+sync rounds) | none | A |
| V8_Python312 | 1 | test_storage passes with bead itself on CPython 3.12 | driver (launcher copy with a 3.12 shebang via BEAD_UNDER_TEST) | none | D: the CI matrix, Linux and macOS on every push and git 2.40 nightly (`.github/workflows/rust-nightly.yml`), kept by `ci::v8_supported_environments` |
| V8_RecoveryFromAPrunedRemote | 1 | Resetting the remote ref to last valid commit converges losslessly | cli (delete loose tip commit; `update-ref` repair) | none | A |
| V9_HarmlessGlobalGitSettings | 3 | Thirteen global git settings stay harmless to sync and claims | cli+env-fake (scratch GIT_CONFIG_GLOBAL; 13 fresh two-machine rounds) | none | A |
| V9_CheckoutHooksDoNotRunOnBeadsPush | 1 | Checkout and husky pre-push hooks never run on bead pushes | cli+env-fake (`.git/hooks`, `.husky`, core.hooksPath) | none | A |
| V9_CallerGitEnvironment | 1 | Caller's GIT_* environment keeps workspace and claims intact | cli+env-fake (GIT_DIR, GIT_WORK_TREE, GIT_INDEX_FILE+GIT_PREFIX, GIT_CONFIG_PARAMETERS, fixed dates, GIT_TRACE) | none | A |
| V9_EveryCheckoutShapeIsOneWorkspace | 1 | Every checkout shape resolves to the same URL-derived workspace | cli (worktrees, bare, shallow, unborn, submodule) | none | A |
| V9_TransportAndUrlSettings | 3 | GIT_SSH_COMMAND, pushInsteadOf and relative origin honoured like git | cli+env-fake (fake `ssh` and refusing core.sshCommand, logging user wrapper; read-only mirror hook; < 30 s) | none | A |
| V5_DataFileModes | 1 | Store files 0600, dirs 0700; checkout files follow the umask | cli+env-fake (umask 022/002/077) + crash hooks | BEAD_TEST_CRASH_BEFORE_PUSH/AFTER_PUSH | A† |
| V5_Retighten | 2 | A write retightens store directories to 0700; reads don't | cli (chmod store dirs) | none | A |
| V5_GitDirInHook | 1 | With GIT_DIR set, sync.git still lands in the store | cli+env-fake (relative and absolute GIT_DIR) | none | A |
| V5_SideFilesIgnored | 1 | Temp, torn and fork side files never read or published | cli | none | A |
| V5_KillAtRename | 1 | SIGKILL before rename keeps targets whole; retry recognised | driver (sitecustomize via PYTHONPATH wraps `os.replace` → SIGKILL self; python3 probe → skip) | kill-at-rename fault hook (target-path substring, like BEAD_KILL_AT_REPLACE) | C |
| V5_SigningConfigIsInert | 1 | Global commit.gpgSign never reaches bead's commits or gpg | cli+env-fake (fake gpg.program) | none | A |
| V2_ReceiptsArePerActor | 1 | Another actor on the same machine never drops a receipt | cli + crash hooks | BEAD_TEST_CRASH_AFTER_PUSH/BEFORE_PUSH | A† |
| V2_InFlightAttemptAcrossActors | 2 | Other actor during unlocked sleep fences or confirms an in-flight release | inproc+patched+env-fake (patched `time.sleep` runs actor B; stalling pre-receive; BEAD_NET_TIMEOUT=1) | pause hook between rounds (lock released, push in flight); BEAD_NET_TIMEOUT | C |
| V2_ConfirmedTwinKeepsTheRetryRecognised | 1 | Identical pending twin confirms; retry recognised, released once | inproc+patched (`drop_pending` raises) then cli | crash hook after local confirm, before pending cleanup | C |
| V1_PushSideTranslation | 2 | Push rejection tokens stay English; races retried under translated git | t1 env-fake only (pure git under 6 locales + hooks, no bead); t2 cli+env-fake (racing pre-receive; `attempts == 2`) | none; the Rust push classifier must keep the same tokens | A |
| V1_StalledReceiveTimeouts | 2 | Stalled or flooding receive returns in time; slow ack recovered | cli+env-fake (pre-receive 200 KiB stderr + `sleep 4`; slow post-receive; BEAD_NET_TIMEOUT=1.5) | BEAD_NET_TIMEOUT | A |
| U06_UrlSchemesAreAscii | 1 | A URL scheme is ASCII: `httpſ://` is not `https://` (Phase 1, U06) | inproc | without_password, normalize_remote API | B |
| V1_RunGitTimeoutHygiene | 1 | run_git timeouts leak no fds or zombies; reap semantics hold | inproc+env-fake (`run_git(timeout=0.2, reap=…)` on a sleeping git alias; /dev/fd count; `ps` zombie scan) | run_git(timeout, reap, check) API | B |
| V6_UserSuppliedPaths | 2 | OSErrors on user-supplied paths are IO; export creates parents | cli+env-fake (missing, dir, chmod 0, loop, too long, ENOTDIR, /dev/stdout) | none | A |
| V6_StoreDamage | 1 | Local store damage gets structured codes, never a crash | cli+env-fake (chmod; lock/segment as dir; pending as file) | none | A |
| V6_FileSizeLimit | 1 | EFBIG is IO or GIT_ERROR; next write and sync clean | cli+env-fake (`ulimit -f 200`) | none (impl: ignore SIGXFSZ; name signals in GIT_ERROR) | A |
| V6_DescriptorExhaustion | 1 | Descriptor exhaustion never drops a pending attempt | cli+env-fake (`ulimit -n 8/10/11`) + crash hook | BEAD_TEST_CRASH_AFTER_PUSH | A† |
| V6_IdentityAndConfigShapes | 1 | replica.json and .beadroll.json shapes validated (refs, workspace ids) | cli | none | A |
| V7_OriginsWithAUserOrSchemeAreOneWorkspace | 2 | Origins with user or scheme give one exact workspace id | t1 cli (id `987c37c1f56322a0`); t2 cli+env-fake (fake ssh, isolated HOME/XDG, NO_PROXY; id `606ef04ef5c6313e`) | none | A |
| V7_PasswordShapesFollowGit | 3 | without_password drops exactly the secrets git would send | t1 inproc; t2 inproc+env-fake (local 401 HTTP recorder, `git ls-remote` oracle); t3 inproc+env-fake (logging GIT_SSH_COMMAND oracle) | without_password API | B |
| V7_NormalizeRemote | 1 | Spellings of one remote share an id; ports stay distinct | inproc | normalize_remote API | B |

## Totals (tests, not classes)

Port class: **A 172** (of which A† 14: G07 3, G08 3, G10 3, G28 2, V5_DataFileModes, V2_ReceiptsArePerActor, V6_DescriptorExhaustion), **B 11**, **B+A split 1** (G32), **C 15**, **D 9**. Sum 208.

Mechanism (primary label per test): cli 98; cli+env-fake 79; env-fake only, no bead 1 (V1_PushSideTranslation t1); cli+inproc 2 (G02 t1, G32); inproc 8; inproc+env-fake 3; inproc+patched 5; inproc+patched+env-fake 3; driver 9. Sum 208. 17 tests use the CLI crash hooks (G07 3, G08 3, G09, G10 3, G28 3, G29 t2, V5_DataFileModes, V2_ReceiptsArePerActor, V6_DescriptorExhaustion).

## Rust-side seams (deduplicated)

1. `BEAD_TEST_CRASH_BEFORE_PUSH` → exit 98 after the attempt (`pending/<id>.json`) and receipt (`pending/<id>.command`) are written, before the push; `BEAD_TEST_CRASH_AFTER_PUSH` → exit 99 after the remote accepted, before local confirm. Coordinated path only: G07, G08, G09, G10, G28, G29, V5_DataFileModes, V2_ReceiptsArePerActor, V6_DescriptorExhaustion.
2. Crash before *any* push, including `publish_soon` (owner close/delete/`update --status closed`): G12.
3. Atomic-write failure injection by target (ENOSPC): meta.json; a foreign segment written during pull: G29.
4. Kill-at-rename (SIGKILL self just before the rename onto a target whose path contains a substring): V5_KillAtRename.
5. Crash after local confirm and before pending cleanup (`drop_pending`): G54, V2_ConfirmedTwinKeepsTheRetryRecognised.
6. Pause hook in the unlocked wait between publish rounds (`pause`; the Python tests patch the global `time.sleep`): G11 t2, V2_InFlightAttemptAcrossActors.
7. Pre-push contention hook: a callback before a claim push, between fetch and push (or a `git` shim on PATH with a recursion guard): G09.
8. Reader hook between listing `pending/` and reading the attempts (`pending_files`): G28 t2.
9. Clock injection for torn/fork fragment names, or names unique even when timestamps are equal: G47 t3. The Python patch of `time.time` does nothing today, because the implementation uses `time_ns`.
10. In-process crate APIs: `Journal.add` / `replay` / State (`issues[uid].deps`, `dangling`, `by_key`, `key()`, `pending`, `integrity`) / `is_blocked` / `read_attempt(path, problems)` / `FORMAT`: G14, G32, G57, V4_DanglingReplayDeterminism. `Workspace(id, name, origin, config)` with `.remote()` / `.save_meta()`, `find_workspace`, `detect_actor`, parser + `cmd_*` entry points: G25, G11, G54, V2. `without_password`, `normalize_remote`: V7, G02. `run_git(args, timeout, reap, check)`: V1_RunGitTimeoutHygiene.
11. Environment contract: `BEAD_NET_TIMEOUT` (float seconds per fetch/push; G08, G58, V1_StalledReceiveTimeouts, V2, V8_SyncUnderAControllingTerminalCannotPrompt); `BEAD_REMOTE` (G18, G58); `BEAD_WORKSPACE` (G11; named in G24's message); `BEAD_HOME` with `~` expansion (G35, G52); `BEAD_AGENT`/`BEAD_THREAD`/`BEAD_ACTOR` (all tests; V3); `CLAUDE_CODE_SESSION_ID` + `CODEX_THREAD_ID` → parent-process walk through `ps` (G34 t5).

## Harness-side fakes the ported tests need

Fake ssh scripts (run the remote command locally, refuse, read-only, log argv, open /dev/tty, keep stderr open through a background child): G05, G18, G20, G58, V7, V8_SyncUnderAControllingTerminalCannotPrompt, V9_TransportAndUrlSettings. Fake gpg: V5_SigningConfigIsInert. Server hooks in the bare remote (refuse, stall on a gate directory, flood stderr, race with `update-ref`, slow post-receive, `kill -9 $PPID`): G08, G11, G18, V1, V2, V9. Client hooks: G19, V9_CheckoutHooksDoNotRunOnBeadsPush. HTTP servers: smart-HTTP through `git http-backend` with header checks (G20); 401→404 recorder (G39); 401 recorder (V7); a closed port (G06). Scratch `GIT_CONFIG_GLOBAL` + `GIT_CONFIG_NOSYSTEM` (about 16 classes). Locales. PTYs, including a controlling terminal through setsid + TIOCSCTTY (G36, G59, V8). Closed fds through `/bin/sh` (G36, G56). `ulimit -f` / `-n` (G48, V6). Umask set in the test process (G47, V5_DataFileModes). chmod 0 / read-only, skipped as root (G03, G28, G50, G52, G53, V6). Remote damage through deleted loose objects and plumbing commits (G30, G34, G41, V3, V8). Maintenance threads (V8).

## Environment variables and external programs

- bead: BEAD_UNDER_TEST, BEAD_HOME, BEAD_AGENT, BEAD_THREAD, BEAD_ACTOR, BEAD_WORKSPACE, BEAD_REMOTE, BEAD_NET_TIMEOUT, BEAD_TEST_CRASH_BEFORE_PUSH, BEAD_TEST_CRASH_AFTER_PUSH. Driver-only: BEAD_KILL_AT_REPLACE, G29_FULL. Harness detection: CLAUDE_CODE_SESSION_ID, CODEX_THREAD_ID. Machine.env strips CLAUDE*, CODEX*, BEAD_*, GIT_* from the inherited environment.
- git: GIT_CONFIG_GLOBAL, GIT_CONFIG_NOSYSTEM, GIT_SSH_COMMAND, GIT_SSH, GIT_SSH_VARIANT, GIT_TERMINAL_PROMPT, GIT_ASKPASS, GIT_DIR, GIT_WORK_TREE, GIT_OBJECT_DIRECTORY, GIT_INDEX_FILE, GIT_PREFIX, GIT_CONFIG_PARAMETERS, GIT_AUTHOR_/COMMITTER_ NAME/EMAIL/DATE, GIT_TRACE, GIT_TEST_ASSUME_DIFFERENT_OWNER, GIT_CEILING_DIRECTORIES, GIT_PROJECT_ROOT, GIT_HTTP_EXPORT_ALL plus the http-backend CGI variables (REMOTE_USER, REQUEST_METHOD, PATH_INFO, QUERY_STRING, CONTENT_TYPE, CONTENT_LENGTH, HTTP_GIT_PROTOCOL, HTTP_CONTENT_ENCODING); hooks unset GIT_QUARANTINE_PATH and GIT_ALTERNATE_OBJECT_DIRECTORIES.
- Locale and terminal: LANG, LC_ALL, LC_MESSAGES, LANGUAGE, TERM, FORCE_COLOR, NO_COLOR, PYTHON_COLORS, PYTHONUTF8, PYTHONCOERCECLOCALE, PYTHONPATH, PYTHONDONTWRITEBYTECODE. Locale names: C, en_US.UTF-8, en_US.ISO8859-1, en_US.US-ASCII, de_DE.UTF-8, fr_FR.UTF-8, fr_FR.ISO8859-1 (the macOS spellings).
- Other: HOME, XDG_CONFIG_HOME, PATH, NO_PROXY/no_proxy (proxy variables removed). Fake-script variables: FAKE_SSH_ROOT, V7_SSH_ROOT, V7_SSH_LOG, G58_PIDS, G58_BG_SECONDS.
- Programs: git, plus `/bin/sh` (`ulimit`, `exec`, `<&-`), sh, `ps` (`-A -o pid=,ppid=,stat=` in the test; bead itself runs `ps -o ppid=,args= -p PID`), chmod, sleep, kill, mkdir, touch, cat, head, tr, printf, `/usr/bin/env`, python3 (as a probe in G13, G59 and V5_KillAtRename), python3.12/3.13 (optional, found on PATH or under `~/.local/share/uv/python`), and Python's `http.server` and `os.openpty`/`os.login_tty`. Devices: /dev/fd, /dev/tty, /dev/zero, /dev/null, /dev/stdout.
- git features: SHA-256 repositories and `rev-parse --show-object-format`; init.defaultObjectFormat; reftable (init.defaultRefFormat, git ≥ 2.45); config-based hooks and `git hook list` (the docstring says ≥ 2.54; skipped otherwise); GIT_TEST_ASSUME_DIFFERENT_OWNER (probe); de/fr message catalogs (probe); `http-backend`; `protocol.file.allow=always` for a local submodule; safe.bareRepository; core.fsmonitor; protocol.version 0/1; upper-case `HTTP://` (git 2.43 rejects it, and V7 t2 then skips). The review ran git 2.55.

## Real time, threads, concurrency

All waits below are estimates read from the code; none was measured.

- Timeouts and sleeps:
  - G08 t2: a 1 s push timeout per round, then `sleep(1)`. About 3 s.
  - G58: BEAD_NET_TIMEOUT=4, a `sleep 15` helper, and a bound of < 4 s.
  - V1_StalledReceiveTimeouts: BEAD_NET_TIMEOUT=1.5 and hooks that sleep 4 s. The tests poll for the hooks to finish. About 10 s.
  - V1_RunGitTimeoutHygiene: 40 timeouts of 0.2 s, 3 s helpers, and 2 × 0.3 s. About 9 s.
  - V2_InFlightAttemptAcrossActors: BEAD_NET_TIMEOUT=1, a gate hook polled every 0.05 s (up to 20 s and 10 s), and 0.3 s sleeps. About 3 s each.
  - V8_SyncUnderAControllingTerminalCannotPrompt: BEAD_NET_TIMEOUT=30 with a bound of < 15 s.
  - V8_HangupOfTheControllingTerminal: up to 30 s.
  - Bounds only: G18 (< 5 s and < 10 s), G36 and G59 (30 s).
- Heavy runs:
  - V8_RemoteMaintenance: 60 operations, `gc --aggressive` after about 45 of them, a maintenance thread, and a wait of up to 20 s for gc.pid.
  - V8_PruneOnThePrivateSyncRepo: 30 rounds while a gc thread runs.
  - V9_HarmlessGlobalGitSettings: 13 sandboxes.
  - V4_DanglingReplayDeterminism: CPU-bound, 2 × 1000 seeds.
  - V8_Python312: runs all of test_storage, with a 600 s cap (12 s measured in the review).
  - V6_StoreDamage: about 60 bead runs.
  - V5_DataFileModes: 3 umasks.
  - V6_DescriptorExhaustion and G41: several sandboxes each.
- Threads: HTTP servers in G20, G39 and V7; a PTY drain thread in G59; maintenance loops in V8_RemoteMaintenance t2 and V8_PruneOnThePrivateSyncRepo; run_git's waiter thread in V1.
- Concurrent processes:
  - G16 runs 7 bead processes at once, in each of 3 rounds.
  - G09, G11 t2, G28 t2 and V2 run bead in-process and start a subprocess bead from inside a hook.
  - The V8 maintenance tests run git maintenance alongside bead.

## D-class tests and the Rust property each becomes

- **G13** (3 tests). Files are always read as UTF-8, whatever the locale. Rust property: the ref "refs/beads/équipe" from .beadroll.json resolves the same under ISO-8859-1, US-ASCII and C locales, and claims on that ref still exclude each other across locales. Keep the tests black-box and drop the python3 probe.
- **G30 t4**. Machine a imports a record 12,000 levels deep and syncs; a peer whose reader stops lower (Python 3.12/3.13 today) keeps working and sees what a sees. Events are bounded to MAX_NESTING = 100 levels and imported records to 98 (the create event holds them two levels down), so the import is refused. In Rust this becomes a mixed-fleet case: Python 3.14 and Rust peers with different reading ceilings (`docs/RUST-PORT.md` §5 row 7).
- **G52 t5**. chmod 0 on the workspace store gives IO for `show` and `info`. Run it unconditionally.
- **G59** (3 tests). Help contains no ESC byte on a PTY, with TERM=xterm-256color, with FORCE_COLOR=1, or in `--json help create`, and it contains "usage: bead create". This matters for clap. Drop the probe.
- **V8_Python312**. Becomes a matrix job: the full suite against the release binary on Linux and macOS and the oldest supported git (musl waits for public binaries, which `docs/RUST-PORT.md` §10 defers).
- Partly Python-specific but kept in another class: G09 (TypeError), the Traceback-free checks in G34/G38/G49/G55/G64, the "Exception ignored" check in G36, and G48 ("Python ignores SIGXFSZ").

## Porter gotchas

1. **Traceback checks.** `assertNotIn("Traceback", stderr)` checks nothing against a Rust binary. Add a check for "panicked at", exit 101, and death by SIGABRT or SIGSEGV (stack overflow).
2. **SIGXFSZ.** Python ignores SIGXFSZ; Rust does not. G48 and V6_FileSizeLimit need EFBIG to become IO. Git children should die "killed by SIGXFSZ", which means bead must reset the default disposition in the child and map signal names in GIT_ERROR. Rust ignores SIGPIPE, and `println!` panics on EPIPE or EIO. On EPIPE, or EIO on a character device, exit 0. Keep exit 1 or 2 when only stderr fails (G36).
3. **Closed stdio.** Rust std reopens closed fds 0, 1 and 2 on /dev/null before `main`. G36's closed stdout still passes. G56 does not: with a closed stdin, `import -` would read empty input and succeed. Detect closed fds before std initialises (`#![no_main]` or a constructor).
4. **Non-UTF-8 arguments.** `std::env::args()` panics on non-UTF-8, and clap rejects it. V3 requires exactly `{"code":"VALIDATION","message":"text is not valid UTF-8"}` with exit 1.
5. **Surrogates.** serde_json refuses `\ud800`, and from_utf8_lossy turns ED A0 80 into three U+FFFD. Python reads both through surrogatepass, which gives exactly one U+FFFD per surrogate: "bad�title", "raw�title", "t�". It also adds the integrity row "text is not valid UTF-8; it reads as U+FFFD" with origin `<replica>/000001.jsonl:1`.
6. **Nesting depth.** MAX_NESTING is 100: depth 200 gives VALIDATION, and depth 50 round-trips byte-exactly. A line 1,000,000 levels deep is one integrity problem ("line is not valid JSON"), never a stack overflow.
7. **Hashes are pinned.** Workspace ids `987c37c1f56322a0` and `606ef04ef5c6313e` are exact, and G02 checks `sha256(normalize_remote(url))[:16]`. For an existing local path the normalised form is realpath plus lowercase. Non-UTF-8 origins are hashed as raw bytes. The 15 `without_password` input/output pairs are exact.
8. **JSON formats.** `--json` output uses Python's default separators (`", "` and `": "`) with ensure_ascii=False. The error envelope is ASCII-escaped. Export is compact (`","`, `":"`) with raw non-ASCII: G32 searches for `"priority":true`, and G31 needs raw U+2028 and exactly 3 `\n`. `export -o` and export to stdout must match byte for byte.
9. **Exact text.**
   - Human output: create's third token is `KEY:`; claim output starts with "Claimed"; errors are printed as `Error: …`.
   - Escaping: control characters become lowercase `\xNN` or `\uNNNN`, including U+061C. PTY output equals pipe output.
   - Message substrings the tests look for: "not yet published", "password was dropped", "meta.json", "BEAD_WORKSPACE=", "refs/beads/journal", "safe.directory", "`git config --global --add safe.directory <checkout>`", "cannot reach the coordination remote", "merged into the default branch", "usage: bead create", "line is not valid JSON", "File too large" or "SIGXFSZ", and the remote's own hook text.
   - Must not appear, where a test pins it: "mint a new" in G53's `IO` error for an unreadable `replica.json` (the `IDENTITY` error does say "mint a new one"), and "no answer" or "not answering" in G18's refused push (a timeout does say "gave no answer").
10. **Store layout.**
    - Machine.store() takes the first directory in BEAD_HOME, so BEAD_HOME must contain only the store directory.
    - Files the tests name: `replica.json` (`{"id"}`), `aliases.json`, `<ws>/meta.json`, `lock`, `sync.git`, `events/<replica>/000001.jsonl`, `events/<r>/claims/<eid>.json`, `pending/<id>.json` and `.command`, `.NAME.PID.tmp` (the identity's is `.replica.PID.tmp`), `*.torn-*`, `*.fork-*`.
    - Modes: everything under BEAD_HOME is 0600 or 0700, sync.git included (`git init --bare --template= --shared=0600 --object-format=<project>`). Checkout files follow the umask. A write retightens the directories to 0700; a read does not.
11. **Pinned git behaviour.**
    - Run `sync.git`'s git with LC_ALL=C (checkout-side git keeps the caller's locale, which G62 t3 relies on), every git in a new session through an audited pre-exec `setsid` call, with output captured to files: wait for git to exit, not for its pipes to close. `process_group(0)` retains the controlling terminal and must not precede `setsid`. Retain V8's controlling-terminal test together with V1's timeout/group-kill checks.
    - On timeout, kill the whole process group for a fetch but only the child for a push.
    - Fetch into a per-process ref, then push with `--porcelain --no-verify`. Classify the result from git's own lines only, never from `remote:` lines.
    - Strip the repository-location variables (GIT_DIR, GIT_WORK_TREE, GIT_OBJECT_DIRECTORY and the rest of REPO_LOCATION_ENV).
    - Carry the checkout's transport configuration through GIT_CONFIG_COUNT without sending any header twice (G20). Set the default GIT_SSH_COMMAND only when the user has set none.
    - Commit with `commit-tree`, so gpgSign is never honoured. Commit subjects are `bead <claim|fence|sync> <hex12>`, and V2 checks their sequence.
12. **Calibration.**
    - `ulimit -n` 8/10/11 is tuned to Python's descriptor use; Rust may need lower limits to reach the same paths.
    - G13, G59 and G52 t5 skip based on Python probes, and those probes would skip or wrongly pass under Rust.
    - V1_PushSideTranslation t1 tests git only. Keep its token list in sync with the Rust classifier.

## Required new port coverage (outside the existing test inventory)

S01's same-store upgrade/rollback receipt tests and S02's integer digit-limit fixtures are new
requirements, described in the commands/protocol and replay/storage catalogues respectively.
G07/G08 preserve the receipt invariant within one implementation; G32 covers typed field ranges.
They do not already cover cross-implementation receipts or the JSON decimal digit limit. The
207-test inventory and its classifications above remain unchanged.
