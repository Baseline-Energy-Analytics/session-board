import json
import os

import pytest

from board import facts

NOW = 1_793_500_000.0  # 2026-10-31


def iso(epoch):
    from datetime import datetime, timezone
    return datetime.fromtimestamp(epoch, timezone.utc).isoformat().replace("+00:00", "Z")


def write_transcript(claude, sid, cwd, prompts=("do the thing",), reply="done it", entry="cli",
                     ts=NOW - 600, root=None, title=None, cleared=False, ai=None, extra=()):
    folder = claude / "projects" / cwd.replace("/", "-")
    folder.mkdir(parents=True, exist_ok=True)
    lines = []
    if cleared:  # what Claude Code writes first in a conversation started with /clear
        lines.append({"type": "attachment", "attachment": {"type": "hook_success", "hookName": "SessionStart:clear"},
                      "sessionId": sid, "timestamp": iso(ts - 1)})
        lines.append({"type": "system", "subtype": "local_command", "commandRun": {"command": "clear", "args": ""},
                      "sessionId": sid, "timestamp": iso(ts - 1)})
    if ai:
        lines.append({"type": "ai-title", "aiTitle": ai, "sessionId": sid})
    if title:
        lines.append({"type": "custom-title", "customTitle": title, "sessionId": sid})
    for i, p in enumerate(prompts):
        lines.append({"type": "user", "uuid": root if (root and i == 0) else f"{sid}-u{i}", "cwd": cwd,
                      "entrypoint": entry, "promptSource": "typed", "timestamp": iso(ts + i),
                      "message": {"role": "user", "content": p}})
        lines.append({"type": "assistant", "cwd": cwd, "entrypoint": entry, "timestamp": iso(ts + i + 0.5),
                      "message": {"role": "assistant", "content": [{"type": "thinking", "thinking": "hm"},
                                                                    {"type": "text", "text": reply}]}})
    lines.extend(extra)
    path = folder / f"{sid}.jsonl"
    path.write_text("\n".join(json.dumps(x) for x in lines) + "\n")
    os.utime(path, (ts, ts))
    return path


def write_job(claude, job, sid, cwd, tempo="idle", detail="", result=None, resume=None, name=None,
              state="done", needs=None):
    d = claude / "jobs" / job
    d.mkdir(parents=True)
    st = {"state": state, "tempo": tempo, "detail": detail, "sessionId": sid, "cwd": cwd,
          "updatedAt": iso(NOW - 60), "resumeSessionId": resume or sid, "name": name}
    if result:
        st["output"] = {"result": result}
    if needs:
        st["needs"] = needs
    (d / "state.json").write_text(json.dumps(st))


@pytest.fixture
def claude(tmp_path):
    return tmp_path / "claude"


def collect(claude, tmp_path, agents):
    return facts.collect(str(claude), str(tmp_path / "cache.json"), agents=agents, now=NOW,
                         deleted_path=str(tmp_path / "deleted.json"))


def by_id(sessions):
    return {s.session_id: s for s in sessions}


def test_job_state_reads_tempo_not_state():
    assert facts.job_state({"state": "blocked", "tempo": "active"}) == "running"
    assert facts.job_state({"state": "done", "tempo": "blocked", "detail": "Approve rm?"}) == "needs you"
    assert facts.job_state({"state": "done", "tempo": "idle"}) == "idle"


def test_freshly_resumed_job_is_idle_not_needs_you():
    st = {"state": "working", "tempo": "blocked", "detail": "(idle — send a prompt to start)"}
    assert facts.job_state(st) == "idle"


def test_states_titles_and_summaries(claude, tmp_path):
    write_transcript(claude, "aaaa1111-job", "/opt/plex", reply="from transcript")
    write_job(claude, "aaaa1111", "aaaa1111-job", "/opt/plex", tempo="blocked", detail="Approve?",
              result="Plex updated.")
    write_transcript(claude, "bbbb-inter", "/opt/plex", prompts=("check logs",), title="Log check")
    write_transcript(claude, "cccc-ended", "/home/x", prompts=("old question",), reply="old answer")
    agents = [
        {"id": "aaaa1111", "kind": "background", "sessionId": "aaaa1111-job", "cwd": "/opt/plex",
         "name": "plex update", "state": "blocked"},
        {"kind": "interactive", "sessionId": "bbbb-inter", "cwd": "/opt/plex", "name": "host-e1", "pid": 1},
    ]
    s = by_id(collect(claude, tmp_path, agents))
    assert s["aaaa1111-job"].state == "needs you"
    assert s["aaaa1111-job"].job == "aaaa1111"
    assert s["aaaa1111-job"].title == "plex update"
    assert s["aaaa1111-job"].summary == "Plex updated."
    assert s["bbbb-inter"].state == "elsewhere"
    assert s["bbbb-inter"].title == "Log check"  # the transcript's title beats the host-made name
    assert s["cccc-ended"].state == "ended"
    assert s["cccc-ended"].asked == "old question"
    assert s["cccc-ended"].said == "old answer"
    assert s["cccc-ended"].title == "old question"


def test_automation_and_promptless_sessions_are_dropped(claude, tmp_path):
    write_transcript(claude, "sdk", "/opt", entry="sdk-py")
    write_transcript(claude, "empty", "/opt", prompts=())
    write_transcript(claude, "real", "/opt")
    assert set(by_id(collect(claude, tmp_path, []))) == {"real"}


def test_old_transcripts_are_skipped(claude, tmp_path):
    write_transcript(claude, "old", "/opt", ts=NOW - 40 * 86400)
    assert collect(claude, tmp_path, []) == []


def test_ended_copy_of_a_live_conversation_is_folded(claude, tmp_path):
    # Backgrounding copies the transcript; both copies keep the first message's uuid.
    write_transcript(claude, "fore", "/opt/ha", root="root-1", ts=NOW - 3600)
    write_transcript(claude, "back", "/opt/ha", root="root-1")
    write_job(claude, "back0000", "back", "/opt/ha")
    agents = [{"id": "back0000", "kind": "background", "sessionId": "back", "cwd": "/opt/ha"}]
    assert set(by_id(collect(claude, tmp_path, agents))) == {"back"}


def test_transcript_a_job_resumed_is_folded(claude, tmp_path):
    write_transcript(claude, "before", "/opt/b", ts=NOW - 3600)
    write_transcript(claude, "after", "/opt/b")
    write_job(claude, "after000", "after", "/opt/b", resume="before")
    agents = [{"id": "after000", "kind": "background", "sessionId": "after", "cwd": "/opt/b"}]
    assert set(by_id(collect(claude, tmp_path, agents))) == {"after"}


def test_two_ended_copies_keep_the_newest(claude, tmp_path):
    write_transcript(claude, "older", "/opt/b", root="r", ts=NOW - 3600)
    write_transcript(claude, "newer", "/opt/b", root="r", ts=NOW - 60)
    assert set(by_id(collect(claude, tmp_path, []))) == {"newer"}


def test_live_sessions_are_never_folded(claude, tmp_path):
    write_transcript(claude, "one", "/opt/b", root="r")
    write_transcript(claude, "two", "/opt/b", root="r")
    write_job(claude, "one00000", "one", "/opt/b")
    write_job(claude, "two00000", "two", "/opt/b")
    agents = [{"id": "one00000", "kind": "background", "sessionId": "one", "cwd": "/opt/b"},
              {"id": "two00000", "kind": "background", "sessionId": "two", "cwd": "/opt/b"}]
    assert set(by_id(collect(claude, tmp_path, agents))) == {"one", "two"}


def test_cache_is_reused_and_private(claude, tmp_path, monkeypatch):
    write_transcript(claude, "s1", "/opt")
    collect(claude, tmp_path, [])
    cache = tmp_path / "cache.json"
    assert oct(cache.stat().st_mode & 0o777) == "0o600"
    calls = []
    monkeypatch.setattr(facts, "parse_transcript", lambda p: calls.append(p) or {})
    collect(claude, tmp_path, [])
    assert calls == []  # unchanged file: served from the cache


def test_project_of(tmp_path):
    home = str(tmp_path / "home")
    repo = tmp_path / "home" / "board"
    (repo / ".git").mkdir(parents=True)
    (repo / "sub").mkdir()
    opt = tmp_path / "opt"
    (opt / ".git").mkdir(parents=True)
    (opt / "plex" / "conf").mkdir(parents=True)
    ws = (str(opt), home)
    facts.git_root.cache_clear()

    def proj(path, scratch="/nonexistent-scratch"):  # pytest's tmp_path is itself under /tmp
        return facts.project_of(path, ws, scratch)
    assert proj(str(repo / "sub")) == str(repo)                    # a repo is one project
    assert proj(str(opt / "plex" / "conf")) == str(opt / "plex")   # a workspace's child is its own
    assert proj(str(opt)) == str(opt)
    assert proj(home) == home
    assert facts.project_of("/tmp/claude-1000/x", ws) == "/tmp"


def test_group_puts_urgent_projects_first():
    S = facts.Session
    sessions = [S("a", "a", "/p1", "/p1", "ended", NOW), S("b", "b", "/p2", "/p2", "needs you", NOW - 9999),
                S("c", "c", "/p1", "/p1", "idle", NOW - 5)]
    groups = facts.group(sessions)
    assert [p for p, _ in groups] == ["/p2", "/p1"]
    assert [s.session_id for s in groups[1][1]] == ["c", "a"]


def test_a_stopped_job_is_flagged(claude, tmp_path):
    write_transcript(claude, "s1", "/opt/a")
    write_job(claude, "j1", "s1", "/opt/a", tempo="idle", detail="stopped", state="stopped")
    write_transcript(claude, "s2", "/opt/a")
    write_job(claude, "j2", "s2", "/opt/a")
    agents = [{"id": "j1", "kind": "background", "sessionId": "s1", "cwd": "/opt/a"},
              {"id": "j2", "kind": "background", "sessionId": "s2", "cwd": "/opt/a"}]
    s = by_id(collect(claude, tmp_path, agents))
    assert (s["s1"].state, s["s1"].stopped) == ("idle", True)
    assert s["s2"].stopped is False


def test_a_deleted_conversation_stays_hidden_until_it_is_used_again(claude, tmp_path):
    # `claude rm` keeps the transcript; both copies of the conversation would come back as ended.
    write_transcript(claude, "copy", "/opt/a", root="R", ts=NOW - 900)
    write_transcript(claude, "gone", "/opt/a", root="R", ts=NOW - 600)
    write_transcript(claude, "other", "/opt/a", ts=NOW - 600)
    facts.forget("gone", "R", str(tmp_path / "deleted.json"), now=NOW - 100)
    assert set(by_id(collect(claude, tmp_path, []))) == {"other"}
    write_transcript(claude, "gone", "/opt/a", root="R", ts=NOW - 50)  # resumed and used since
    assert set(by_id(collect(claude, tmp_path, []))) == {"other", "gone"}


def test_a_deleted_live_session_still_shows(claude, tmp_path):
    write_transcript(claude, "back", "/opt/a", root="R", ts=NOW - 900)
    write_job(claude, "j1", "back", "/opt/a")
    facts.forget("back", "R", str(tmp_path / "deleted.json"), now=NOW)
    agents = [{"id": "j1", "kind": "background", "sessionId": "back", "cwd": "/opt/a"}]
    assert set(by_id(collect(claude, tmp_path, agents))) == {"back"}


def test_forget_drops_entries_older_than_the_transcripts(tmp_path):
    path = str(tmp_path / "state" / "deleted.json")
    facts.forget("old", None, path, now=NOW - 40 * 86400)
    facts.forget("new", "R", path, now=NOW)
    assert facts.load_deleted(path) == {"new": NOW, "R": NOW}
    assert oct(os.stat(path).st_mode & 0o777) == "0o600"


def test_a_damaged_deleted_file_is_ignored(tmp_path):
    path = tmp_path / "deleted.json"
    path.write_text('{"a": "soon", "b": 5}')
    assert facts.load_deleted(str(path)) == {"b": 5.0}
    path.write_text("not json")
    assert facts.load_deleted(str(path)) == {}


# --- /clear in a background session ------------------------------------------------------------

def cleared_job(claude):
    # Job j1 began as conversation "first"; /clear moved it on to "second", which carries the job's
    # name as an inherited title. The job still reports sessionId "first".
    write_transcript(claude, "first", "/opt/a", prompts=("what is rust for",), ai="Rust uses", ts=NOW - 900)
    write_transcript(claude, "second", "/opt/a", prompts=("read the handoff",), ai="Rust uses",
                     cleared=True, ts=NOW - 300)
    write_job(claude, "j1", "first", "/opt/a", resume="second", name="Rust uses")
    return [{"id": "j1", "kind": "background", "sessionId": "first", "cwd": "/opt/a", "name": "Rust uses"}]


def test_a_cleared_job_shows_its_current_conversation(claude, tmp_path):
    s = by_id(collect(claude, tmp_path, cleared_job(claude)))
    assert set(s) == {"second", "first"}
    assert (s["second"].job, s["second"].title, s["second"].asked) == ("j1", "Rust uses", "read the handoff")
    assert (s["first"].state, s["first"].title) == ("ended", "Rust uses (before /clear)")


def test_a_cleared_conversation_continued_in_a_terminal_shows_once(claude, tmp_path):
    agents = cleared_job(claude) + [{"kind": "interactive", "sessionId": "second", "cwd": "/opt/a",
                                     "name": "Rust uses"}]
    s = by_id(collect(claude, tmp_path, agents))
    assert set(s) == {"second", "first"}
    assert (s["second"].state, s["second"].job, s["second"].title) == ("elsewhere", None, "Rust uses")


def test_a_background_copy_is_not_mistaken_for_a_clear(claude, tmp_path):
    # `claude --bg --resume` of a running session makes a copy; resumeSessionId names the original.
    write_transcript(claude, "orig", "/opt/a", root="R", ts=NOW - 900)
    write_transcript(claude, "copy", "/opt/a", root="R", ts=NOW - 300)
    write_job(claude, "j1", "copy", "/opt/a", resume="orig")
    agents = [{"id": "j1", "kind": "background", "sessionId": "copy", "cwd": "/opt/a"}]
    s = by_id(collect(claude, tmp_path, agents))
    assert set(s) == {"copy"} and s["copy"].job == "j1"


def test_an_ended_conversation_started_with_clear_is_titled_by_its_first_prompt(claude, tmp_path):
    write_transcript(claude, "x", "/opt/a", prompts=("fix the cert",), ai="Old session name", cleared=True)
    assert by_id(collect(claude, tmp_path, []))["x"].title == "fix the cert"


# --- stale jobs --------------------------------------------------------------------------------

def write_roster(claude, workers):
    """Claude Code's list of running background workers. A worker given as True gets this test's
    own pid and start time, so it reads as alive; False gets a pid that can't exist."""
    d = claude / "daemon"
    d.mkdir(parents=True, exist_ok=True)
    with open(f"/proc/{os.getpid()}/stat") as f:
        start = f.read().rsplit(")", 1)[1].split()[19]
    entries = {job: {"pid": os.getpid() if alive else 2 ** 22 + 7, "procStart": start}
               for job, alive in workers.items()}
    (d / "roster.json").write_text(json.dumps({"proto": 1, "supervisorPid": 1, "workers": entries}))


def stale_jobs(claude):
    write_transcript(claude, "ask", "/opt/a")
    write_job(claude, "askjob", "ask", "/opt/a", tempo="blocked", detail="Approve?", needs="approve rm -rf build")
    write_transcript(claude, "busy", "/opt/b")
    write_job(claude, "busyjob", "busy", "/opt/b", tempo="active")
    write_transcript(claude, "rest", "/opt/c")
    write_job(claude, "restjob", "rest", "/opt/c", tempo="idle")
    write_transcript(claude, "halt", "/opt/d")
    write_job(claude, "haltjob", "halt", "/opt/d", tempo="idle", state="stopped")
    return [{"id": j, "kind": "background", "sessionId": s} for j, s in
            (("askjob", "ask"), ("busyjob", "busy"), ("restjob", "rest"), ("haltjob", "halt"))]


def test_a_job_with_no_process_is_stale_not_needing_you(claude, tmp_path):
    agents = stale_jobs(claude)
    write_roster(claude, {"busyjob": False})  # listed, but its process is gone
    s = by_id(collect(claude, tmp_path, agents))
    assert (s["ask"].state, s["ask"].was) == ("stale", "needs you")
    assert (s["busy"].state, s["busy"].was) == ("stale", "running")
    assert s["rest"].state == "idle" and s["halt"].state == "idle"  # nothing claimed, nothing stale
    assert s["ask"].waiting == "approve rm -rf build"  # what it last needed, kept for the row


def test_a_job_with_a_live_worker_keeps_its_state(claude, tmp_path):
    agents = stale_jobs(claude)
    write_roster(claude, {"askjob": True, "busyjob": True})
    s = by_id(collect(claude, tmp_path, agents))
    assert (s["ask"].state, s["ask"].was) == ("needs you", None)
    assert s["busy"].state == "running"


def test_without_a_roster_nothing_is_called_stale(claude, tmp_path):
    s = by_id(collect(claude, tmp_path, stale_jobs(claude)))
    assert s["ask"].state == "needs you" and s["busy"].state == "running"


# --- what a session is waiting on ----------------------------------------------------------------

def tool_use(tid, name, args, ts=NOW - 100):
    return {"type": "assistant", "timestamp": iso(ts), "message": {
        "id": f"msg-{tid}", "role": "assistant", "content": [{"type": "tool_use", "id": tid, "name": name, "input": args}]}}


def tool_result(tid, ts=NOW - 90):
    return {"type": "user", "timestamp": iso(ts), "message": {
        "role": "user", "content": [{"type": "tool_result", "tool_use_id": tid, "content": "ok"}]}}


def test_waiting_on_prefers_the_jobs_own_words(claude, tmp_path):
    write_transcript(claude, "j", "/opt/a", extra=[tool_use("t1", "Bash", {"command": "make"})])
    write_job(claude, "jj", "j", "/opt/a", tempo="blocked", needs="Reply yes   to commit")
    assert by_id(collect(claude, tmp_path, [{"id": "jj", "kind": "background", "sessionId": "j"}]))["j"].waiting \
        == "Reply yes to commit"


def test_waiting_on_falls_back_to_the_pending_tool_call(claude, tmp_path):
    write_transcript(claude, "j", "/opt/a", extra=[
        tool_use("t1", "Read", {"file_path": "/opt/a/x"}), tool_result("t1"),
        tool_use("t2", "Bash", {"command": "curl -H 'X: 1' https://api?x=1\nAPI_KEY=sk-123456 deploy"}, ts=NOW - 80)])
    write_job(claude, "jj", "j", "/opt/a", tempo="blocked", detail="Approve?")
    s = by_id(collect(claude, tmp_path, [{"id": "jj", "kind": "background", "sessionId": "j"}]))["j"]
    assert s.waiting == "Bash: curl -H 'X: 1' https://api?x=1 API_KEY=… deploy"


def test_a_question_is_shown_as_its_text(claude, tmp_path):
    write_transcript(claude, "j", "/opt/a", extra=[
        tool_use("q1", "AskUserQuestion", {"questions": [{"question": "Which repo?", "options": []}]})])
    write_job(claude, "jj", "j", "/opt/a", tempo="blocked")
    assert by_id(collect(claude, tmp_path, [{"id": "jj", "kind": "background", "sessionId": "j"}]))["j"].waiting \
        == "Which repo?"


def test_a_session_not_waiting_has_no_waiting_line(claude, tmp_path):
    write_transcript(claude, "j", "/opt/a", extra=[tool_use("t1", "Bash", {"command": "ls"})])
    write_job(claude, "jj", "j", "/opt/a", tempo="active")
    assert by_id(collect(claude, tmp_path, [{"id": "jj", "kind": "background", "sessionId": "j"}]))["j"].waiting is None


def test_a_new_prompt_clears_an_unanswered_tool_call(tmp_path):
    path = write_transcript(tmp_path, "j", "/opt/a", extra=[
        tool_use("t1", "Bash", {"command": "ls"}),
        {"type": "user", "promptSource": "typed", "timestamp": iso(NOW - 50),
         "message": {"role": "user", "content": "never mind"}}])
    assert facts.parse_transcript(str(path))["pending"] is None


def test_mask_secrets():
    m = facts.mask_secrets
    assert m("API_KEY=abc123 curl x") == "API_KEY=… curl x"
    assert m("mysql --password=hunter2 -u root") == "mysql --password=… -u root"
    assert m("git clone https://ryan:ghp_abcd@github.com/x") == "git clone https://ryan:…@github.com/x"
    assert "sk-ant-abcdefgh12345" not in m('curl -H "Authorization: Bearer sk-ant-abcdefgh12345" x')
    assert m("echo " + "a1" * 20) == "echo …"
    assert m("docker compose restart homeassistant") == "docker compose restart homeassistant"


# --- model and tokens ----------------------------------------------------------------------------

def reply(mid, model, inp, cache, out, ts):
    return {"type": "assistant", "timestamp": iso(ts), "message": {
        "id": mid, "model": model, "role": "assistant", "content": [{"type": "text", "text": "x"}],
        "usage": {"input_tokens": inp, "cache_creation_input_tokens": 0, "cache_read_input_tokens": cache,
                  "output_tokens": out}}}


def test_model_context_and_output_tokens(tmp_path):
    path = write_transcript(tmp_path, "j", "/opt/a", extra=[
        reply("m1", "claude-opus-5-5", 10, 40_000, 300, NOW - 30),
        reply("m1", "claude-opus-5-5", 10, 40_000, 900, NOW - 29),   # the same reply, a later block
        reply("m2", "claude-sonnet-5-5", 5, 182_000, 100, NOW - 20),
        reply("m3", "<synthetic>", 0, 0, 0, NOW - 10)])               # Claude Code's own notices
    t = facts.parse_transcript(str(path))
    assert (t["model"], t["context"], t["output"]) == ("claude-sonnet-5-5", 182_005, 1000)


def test_liveness_without_proc_falls_back_to_the_pid(monkeypatch):
    real_isdir = os.path.isdir
    monkeypatch.setattr(facts.os.path, "isdir", lambda p: False if p == "/proc/self" else real_isdir(p))
    assert facts._alive(os.getpid(), "anything")
    assert not facts._alive(2 ** 22 + 7, "x")
    assert not facts._alive("nope", "x") and not facts._alive(0, "x")
