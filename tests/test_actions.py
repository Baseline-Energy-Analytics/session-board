import subprocess

import pytest

from board import actions
from board.facts import Session


def session(**kw):
    base = dict(session_id="abcdef12-3456", title="t", cwd="/", project="/", state="ended", last=0.0)
    return Session(**{**base, **kw})


def test_background_session_attaches():
    assert actions.plan_enter(session(state="idle", job="abcdef12")) == actions.Attach("abcdef12")


def test_ended_session_resumes_in_its_folder(tmp_path):
    s = session(cwd=str(tmp_path))
    assert actions.plan_enter(s) == actions.ResumeThenAttach("abcdef12-3456", str(tmp_path))


def test_elsewhere_is_refused():
    with pytest.raises(actions.Refused, match="another terminal"):
        actions.plan_enter(session(state="elsewhere"))


def test_ended_session_in_a_deleted_folder_is_refused():
    with pytest.raises(actions.Refused, match="no longer exists"):
        actions.plan_enter(session(cwd="/nonexistent/folder"))


def fake_run(stdout="", returncode=0, stderr=""):
    calls = []

    def run(argv, **kw):
        calls.append((argv, kw["cwd"]))
        return subprocess.CompletedProcess(argv, returncode, stdout, stderr)
    return run, calls


def test_resume_parses_the_short_id():
    run, calls = fake_run("Starting background service…\nbackgrounded · 9e3c0f69 (idle — send a prompt to start)\n")
    assert actions.resume_in_background("9e3c0f69-cdfd", "/opt", run=run) == "9e3c0f69"
    assert calls == [(["claude", "--bg", "--resume", "9e3c0f69-cdfd"], "/opt")]


def test_resume_falls_back_to_the_id_prefix():
    run, _ = fake_run("something new\n")
    assert actions.resume_in_background("9e3c0f69-cdfd", "/opt", run=run) == "9e3c0f69"


def test_resume_failure_is_refused_with_the_reason():
    run, _ = fake_run(returncode=1, stderr="No conversation found")
    with pytest.raises(actions.Refused, match="No conversation found"):
        actions.resume_in_background("x", "/opt", run=run)


# --- new, stop, delete ---------------------------------------------------------------------------

def test_plan_new_expands_the_folder_and_tidies_the_name(tmp_path):
    req = actions.plan_new(f" {tmp_path} ", "  fix   the  cert ", " go \n")
    assert req == actions.NewSession(str(tmp_path), "fix the cert", "go")


def test_plan_new_expands_tilde():
    assert actions.plan_new("~", "x").cwd == actions.HOME


def test_plan_new_refuses_a_missing_folder():
    with pytest.raises(actions.Refused, match="no folder /nonexistent/folder"):
        actions.plan_new("/nonexistent/folder", "x")


def test_plan_new_needs_a_name_or_a_prompt(tmp_path):
    with pytest.raises(actions.Refused, match="name or a first prompt"):
        actions.plan_new(str(tmp_path), " ", "")
    assert actions.plan_new(str(tmp_path), "", "do it").prompt == "do it"


def test_new_argv_keeps_a_dash_prompt_from_being_read_as_an_option():
    req = actions.NewSession("/opt", "n", "-x do it")
    assert actions.new_argv(req) == ["--bg", "--name", "n", "--", "-x do it"]
    assert actions.new_argv(actions.NewSession("/opt", "n")) == ["--bg", "--name", "n"]
    assert actions.new_argv(actions.NewSession("/opt", "", "p")) == ["--bg", "--", "p"]


def test_start_parses_the_short_id_and_runs_in_the_folder():
    run, calls = fake_run("backgrounded · 83297fbd · sb-test (idle — send a prompt to start)\n"
                          "  claude attach 83297fbd    open in this terminal\n")
    assert actions.start_in_background(actions.NewSession("/srv", "sb-test"), run=run) == "83297fbd"
    assert calls == [(["claude", "--bg", "--name", "sb-test"], "/srv")]


def test_start_without_an_id_is_refused():
    run, _ = fake_run("something new\n")
    with pytest.raises(actions.Refused, match="couldn't read its id"):
        actions.start_in_background(actions.NewSession("/srv", "x"), run=run)


def test_stop_and_delete_run_the_cli():
    run, calls = fake_run("stopped 83297fbd\n")
    actions.stop_background("83297fbd", run=run)
    actions.delete_background("83297fbd", run=run)
    assert [c[0] for c in calls] == [["claude", "stop", "83297fbd"], ["claude", "rm", "83297fbd"]]


def test_delete_failure_carries_the_cli_message():
    run, _ = fake_run(returncode=1, stdout="No job matching '83297fbd'\n")
    with pytest.raises(actions.Refused, match="No job matching"):
        actions.delete_background("83297fbd", run=run)


@pytest.mark.parametrize("kw, words", [
    (dict(state="elsewhere"), "stop it there"),
    (dict(state="ended"), "nothing to stop"),
    (dict(state="idle", job="j", stopped=True), "already stopped"),
])
def test_plan_stop_refuses(kw, words):
    with pytest.raises(actions.Refused, match=words):
        actions.plan_stop(session(**kw))


def test_plan_stop_and_delete_return_the_job():
    s = session(state="running", job="j")
    assert actions.plan_stop(s) == actions.plan_delete(s) == "j"
    assert actions.plan_delete(session(state="idle", job="j", stopped=True)) == "j"


@pytest.mark.parametrize("kw, words", [
    (dict(state="elsewhere"), "another terminal"),
    (dict(state="ended"), "saved conversation"),
])
def test_plan_delete_refuses(kw, words):
    with pytest.raises(actions.Refused, match=words):
        actions.plan_delete(session(**kw))
