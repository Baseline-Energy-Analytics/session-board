import asyncio
import time

from textual.widgets import Input, Static

from board import actions
from board.app import Board, ConfirmScreen, NewSessionScreen, SessionTree, session_label, short_path
from board.facts import Session

NOW = time.time()


def sessions(tmp_path):
    return [
        Session("job-1", "plex update", "/opt/plex", "/opt/plex", "needs you", NOW - 60, job="job1",
                asked="update plex", summary="Waiting for approval to restart."),
        Session("old-1", "old chat", str(tmp_path), "/opt/plex", "ended", NOW - 7200,
                asked="what broke", said="the cert expired"),
        Session("int-1", "here now", "/home/x", "/home/x", "elsewhere", NOW),
        Session("old-2", "shop question", "/home/x/shop", "/home/x/shop", "ended", NOW - 86400),
        Session("run-1", "media build", "/opt/media", "/opt/media", "running", NOW - 30, job="run1"),
        Session("stop-1", "paused thing", "/opt/media", "/opt/media", "idle", NOW - 3000, job="stp1",
                stopped=True),
        Session("tmp-1", "scratch thing", str(tmp_path), "/tmp", "ended", NOW - 90000),
    ]


class FakeRunner:
    def __init__(self, fail=None, on_new=None):
        self.calls = []
        self.fail = fail or {}
        self.on_new = on_new

    def _maybe_fail(self, what):
        if what in self.fail:
            raise actions.Refused(self.fail[what])

    def resume(self, session_id, cwd):
        self.calls.append(("resume", session_id, cwd))
        return session_id[:4]

    def new(self, req):
        self.calls.append(("new", req))
        self._maybe_fail("new")
        if self.on_new:
            self.on_new(req)
        return "new1"

    def stop(self, job):
        self.calls.append(("stop", job))
        self._maybe_fail("stop")

    def delete(self, s, job):
        self.calls.append(("rm", job))
        self._maybe_fail("rm")

    def attach(self, app, job):
        self.calls.append(("attach", job))


def run(scenario, tmp_path, data=None, runner=None, show_ended=True):
    async def go():
        nonlocal runner
        runner = runner or FakeRunner()
        rows = data if data is not None else sessions(tmp_path)
        app = Board(source=lambda: list(rows), runner=runner, poll=None, show_ended=show_ended)
        async with app.run_test(size=(120, 40)) as pilot:
            await app.workers.wait_for_complete()
            await pilot.pause()
            await scenario(app, pilot, runner)
    asyncio.run(go())


async def settle(app, pilot):
    await pilot.pause()
    await app.workers.wait_for_complete()
    await pilot.pause()
    await pilot.pause()


async def goto(app, pilot, data):
    """Put the cursor on the row whose data is `data`, opening its project first."""
    tree = app.query_one(SessionTree)
    for p in tree.root.children:
        if p.data == data:
            tree.move_cursor(p)
            await pilot.pause()
            return
        for n in p.children:
            if n.data == data:
                p.expand()
                await pilot.pause()
                tree.move_cursor(n)
                await pilot.pause()
                return
    raise AssertionError(f"no row {data}")


def notices(app):
    return " | ".join(str(n.message) for n in app._notifications)


def labels(app):
    tree = app.query_one(SessionTree)
    out = []

    def walk(node, depth):
        for child in node.children:
            out.append(("  " * depth) + str(child.label) + (" [open]" if child.is_expanded else ""))
            if child.is_expanded:
                walk(child, depth + 1)
    walk(tree.root, 0)
    return out


def cursor(app):
    return app.query_one(SessionTree).cursor_node.data


def test_projects_with_live_sessions_open_first(tmp_path):
    async def scenario(app, pilot, runner):
        rows = labels(app)
        assert rows[0].startswith("plex") and rows[0].endswith("[open]")
        assert any("plex update" in r for r in rows)
        assert not any("shop question" in r for r in rows)  # only ended sessions: stays closed
    run(scenario, tmp_path)


def test_expand_a_session_to_read_its_summary(tmp_path):
    async def scenario(app, pilot, runner):
        await pilot.press("down", "right")  # project row → first session → open it
        assert cursor(app) == ("session", "job-1")
        rows = "\n".join(labels(app))
        assert "Asked: update plex" in rows
        assert "Status: Waiting for approval to restart." in rows
    run(scenario, tmp_path)


def test_enter_attaches_a_background_session(tmp_path):
    async def scenario(app, pilot, runner):
        await pilot.press("down", "enter")
        await app.workers.wait_for_complete()
        assert runner.calls == [("attach", "job1")]
    run(scenario, tmp_path)


def test_enter_resumes_then_attaches_an_ended_session(tmp_path):
    async def scenario(app, pilot, runner):
        await pilot.press("down", "down", "enter")
        assert runner.calls == [("resume", "old-1", str(tmp_path)), ("attach", "old-")]
    run(scenario, tmp_path)


def test_enter_on_a_session_elsewhere_only_explains(tmp_path):
    async def scenario(app, pilot, runner):
        tree = app.query_one(SessionTree)
        node = next(n for p in tree.root.children for n in p.children if n.data == ("session", "int-1"))
        tree.move_cursor(node)
        await pilot.press("enter")
        await pilot.pause()
        assert runner.calls == []
        assert any("another terminal" in n.message for n in app._notifications)
    run(scenario, tmp_path)


def test_enter_on_a_project_toggles_it(tmp_path):
    async def scenario(app, pilot, runner):
        await pilot.press("enter")
        assert not labels(app)[0].endswith("[open]")
        await pilot.press("enter")
        assert labels(app)[0].endswith("[open]")
        assert runner.calls == []
    run(scenario, tmp_path)


def test_coming_back_keeps_the_cursor_and_open_folds(tmp_path):
    async def scenario(app, pilot, runner):
        await pilot.press("down", "right", "enter")  # open the summary, then jump in
        await app.workers.wait_for_complete()
        await pilot.pause()
        await pilot.pause()
        assert runner.calls == [("attach", "job1")]
        assert cursor(app) == ("session", "job-1")
        assert "Asked: update plex" in "\n".join(labels(app))
    run(scenario, tmp_path)


def test_left_goes_to_the_parent(tmp_path):
    async def scenario(app, pilot, runner):
        await pilot.press("down", "left")
        assert cursor(app)[0] == "project"
    run(scenario, tmp_path)


def test_project_that_comes_alive_opens_unless_ryan_closed_it(tmp_path):
    data = sessions(tmp_path)

    async def go():
        app = Board(source=lambda: list(data), runner=FakeRunner(), poll=None, show_ended=True)
        async with app.run_test(size=(120, 40)) as pilot:
            await app.workers.wait_for_complete()
            await pilot.pause()
            assert any(r.startswith("shop") and not r.endswith("[open]") for r in labels(app))
            data[3] = Session("old-2", "shop question", "/home/x/shop", "/home/x/shop", "running", NOW)
            await pilot.press("enter")  # you close plex, the first project
            await pilot.press("r")
            await app.workers.wait_for_complete()
            await pilot.pause()
            rows = labels(app)
            assert any(r.startswith("shop") and r.endswith("[open]") for r in rows)
            assert any(r.startswith("plex") and not r.endswith("[open]") for r in rows)
    asyncio.run(go())


# --- s: stop ---------------------------------------------------------------------------------------

def test_s_stops_a_session_that_isnt_working_without_asking(tmp_path):
    async def scenario(app, pilot, runner):
        await goto(app, pilot, ("session", "job-1"))
        await pilot.press("s")
        await settle(app, pilot)
        assert runner.calls == [("stop", "job1")]
        assert "Stopped “plex update”" in notices(app)
    run(scenario, tmp_path)


def test_s_asks_first_when_the_session_is_working(tmp_path):
    async def scenario(app, pilot, runner):
        await goto(app, pilot, ("session", "run-1"))
        await pilot.press("s")
        await pilot.pause()
        assert isinstance(app.screen, ConfirmScreen)
        await pilot.press("n")
        await pilot.pause()
        assert runner.calls == [] and not isinstance(app.screen, ConfirmScreen)
        await pilot.press("s")
        await pilot.pause()
        await pilot.press("y")
        await settle(app, pilot)
        assert runner.calls == [("stop", "run1")]
    run(scenario, tmp_path)


def test_s_refuses_a_stopped_or_ended_session(tmp_path):
    async def scenario(app, pilot, runner):
        await goto(app, pilot, ("session", "stop-1"))
        await pilot.press("s")
        await goto(app, pilot, ("session", "old-1"))
        await pilot.press("s")
        await pilot.pause()
        assert runner.calls == []
        assert "already stopped" in notices(app) and "nothing to stop" in notices(app)
    run(scenario, tmp_path)


def test_s_on_a_project_row_explains(tmp_path):
    async def scenario(app, pilot, runner):
        await pilot.press("s", "x")
        await pilot.pause()
        assert runner.calls == []
        assert "Highlight a session" in notices(app)
    run(scenario, tmp_path)


def test_a_failed_stop_is_reported(tmp_path):
    async def scenario(app, pilot, runner):
        await goto(app, pilot, ("session", "job-1"))
        await pilot.press("s")
        await settle(app, pilot)
        assert "Couldn't stop: gone" in notices(app)
    run(scenario, tmp_path, runner=FakeRunner(fail={"stop": "Couldn't stop: gone"}))


def test_a_stopped_session_says_so():
    s = Session("a", "paused", "/opt", "/opt", "idle", NOW, job="j", stopped=True)
    assert "stopped ·" in str(session_label(s, 100))


# --- x: delete -------------------------------------------------------------------------------------

def test_x_deletes_only_after_yes(tmp_path):
    async def scenario(app, pilot, runner):
        await goto(app, pilot, ("session", "job-1"))
        await pilot.press("x")
        await pilot.pause()
        assert isinstance(app.screen, ConfirmScreen)
        assert "plex update" in str(app.screen.message)
        await pilot.press("escape")
        await pilot.pause()
        await pilot.press("x")
        await pilot.pause()
        await pilot.press("enter")  # the focus starts on Cancel
        await pilot.pause()
        assert runner.calls == []
        await pilot.press("x")
        await pilot.pause()
        await pilot.press("y")
        await settle(app, pilot)
        assert runner.calls == [("rm", "job1")]
        assert "Deleted “plex update”" in notices(app)
    run(scenario, tmp_path)


def test_x_warns_when_the_session_is_working(tmp_path):
    async def scenario(app, pilot, runner):
        await goto(app, pilot, ("session", "run-1"))
        await pilot.press("x")
        await pilot.pause()
        assert "working right now" in app.screen.message
    run(scenario, tmp_path)


def test_x_refuses_an_ended_session_and_one_elsewhere(tmp_path):
    async def scenario(app, pilot, runner):
        await goto(app, pilot, ("session", "old-1"))
        await pilot.press("x")
        await goto(app, pilot, ("session", "int-1"))
        await pilot.press("x")
        await pilot.pause()
        assert runner.calls == [] and not isinstance(app.screen, ConfirmScreen)
        assert "saved conversation" in notices(app) and "another terminal" in notices(app)
    run(scenario, tmp_path)


def test_q_in_a_dialog_does_not_quit(tmp_path):
    async def scenario(app, pilot, runner):
        await goto(app, pilot, ("session", "job-1"))
        await pilot.press("x")
        await pilot.pause()
        await pilot.press("q", "r")
        await pilot.pause()
        assert isinstance(app.screen, ConfirmScreen) and app.is_running
        await pilot.press("n")
    run(scenario, tmp_path)


# --- n: new ----------------------------------------------------------------------------------------

async def type_text(pilot, text):
    await pilot.press(*[{" ": "space", "-": "minus"}.get(c, c) for c in text])


def test_n_starts_a_session_in_the_highlighted_sessions_folder_and_jumps_in(tmp_path):
    data = sessions(tmp_path)

    def on_new(req):
        data.append(Session("new-s", "fresh", req.cwd, "/opt/plex", "idle", NOW, job="new1"))

    async def scenario(app, pilot, runner):
        await goto(app, pilot, ("session", "old-1"))
        await pilot.press("n")
        await pilot.pause()
        assert isinstance(app.screen, NewSessionScreen)
        assert app.screen.query_one("#folder", Input).value == short_path(str(tmp_path))
        await type_text(pilot, "fresh")
        await pilot.press("enter")
        await type_text(pilot, "say hi")
        await pilot.press("enter")
        await settle(app, pilot)
        assert runner.calls == [("new", actions.NewSession(str(tmp_path), "fresh", "say hi")),
                                ("attach", "new1")]
        assert app.query_one(SessionTree).cursor_node.data == ("session", "new-s")
    run(scenario, tmp_path, data=data, runner=FakeRunner(on_new=on_new))


def test_n_on_a_project_row_offers_the_projects_folder(tmp_path):
    async def scenario(app, pilot, runner):
        await goto(app, pilot, ("project", "/home/x/shop"))
        await pilot.press("n")
        await pilot.pause()
        # /home/x/shop doesn't exist here, so it falls back to home
        assert app.screen.query_one("#folder", Input).value == "~"
        await pilot.press("escape")
        await pilot.pause()
        assert runner.calls == [] and not isinstance(app.screen, NewSessionScreen)
    run(scenario, tmp_path)


def test_n_on_the_scratch_project_offers_its_latest_sessions_folder(tmp_path):
    async def scenario(app, pilot, runner):
        await goto(app, pilot, ("project", "/tmp"))
        await pilot.press("n")
        await pilot.pause()
        assert app.screen.query_one("#folder", Input).value == short_path(str(tmp_path))
    run(scenario, tmp_path)


def test_n_explains_what_is_wrong_and_stays_open(tmp_path):
    async def scenario(app, pilot, runner):
        await goto(app, pilot, ("session", "old-1"))
        await pilot.press("n")
        await pilot.pause()
        await pilot.press("enter", "enter")  # no name, no prompt
        await pilot.pause()
        assert isinstance(app.screen, NewSessionScreen)
        assert "name or a first prompt" in str(app.screen.query_one("#error", Static).render())
        app.screen.query_one("#folder", Input).value = "/nonexistent/x"
        app.screen.query_one("#name", Input).value = "y"
        app.screen.submit()
        await pilot.pause()
        assert "no folder /nonexistent/x" in str(app.screen.query_one("#error", Static).render())
        assert runner.calls == []
    run(scenario, tmp_path)


def test_a_failed_start_is_reported_and_nothing_attaches(tmp_path):
    async def scenario(app, pilot, runner):
        await goto(app, pilot, ("session", "old-1"))
        await pilot.press("n")
        await pilot.pause()
        await type_text(pilot, "x")
        await pilot.press("enter", "enter")
        await settle(app, pilot)
        assert [c[0] for c in runner.calls] == ["new"]
        assert "Couldn't start: no auth" in notices(app)
    run(scenario, tmp_path, runner=FakeRunner(fail={"new": "Couldn't start: no auth"}))


# --- /: filter -------------------------------------------------------------------------------------

def test_filter_shows_matches_with_their_projects_open(tmp_path):
    async def scenario(app, pilot, runner):
        await pilot.press("slash")
        await type_text(pilot, "shop")
        await pilot.pause()
        rows = labels(app)
        assert rows[0].startswith("shop") and rows[0].endswith("[open]")  # opened though it's ended
        assert any("shop question" in r for r in rows)
        assert not any("plex" in r for r in rows)
    run(scenario, tmp_path)


def test_filter_matches_folders_and_every_word(tmp_path):
    async def scenario(app, pilot, runner):
        await pilot.press("slash")
        await type_text(pilot, "opt media")
        await pilot.pause()
        rows = "\n".join(labels(app))
        assert "media build" in rows and "paused thing" in rows and "plex" not in rows
    run(scenario, tmp_path)


def test_filter_survives_a_reload_and_esc_clears_it(tmp_path):
    async def scenario(app, pilot, runner):
        await pilot.press("slash")
        await type_text(pilot, "shop")
        await pilot.press("enter")  # back to the list, filter kept
        await pilot.press("r")
        await settle(app, pilot)
        assert not any("plex" in r for r in labels(app))
        assert "filter: shop" in app.sub_title
        await pilot.press("escape")
        await pilot.pause()
        assert any(r.startswith("plex") for r in labels(app))
        assert not app.query_one("#filter", Input).has_class("shown")
        assert app.query_one(SessionTree).has_focus
    run(scenario, tmp_path)


def test_keys_typed_into_the_filter_stay_there(tmp_path):
    async def scenario(app, pilot, runner):
        await pilot.press("slash")
        await type_text(pilot, "qnsxr")
        await pilot.pause()
        assert app.is_running and app.screen is app.screen_stack[0]
        assert app.query_one("#filter", Input).value == "qnsxr"
        assert "No sessions match" in "\n".join(labels(app))
        assert runner.calls == []
    run(scenario, tmp_path)


def test_a_project_closed_while_filtering_stays_closed_until_the_filter_changes(tmp_path):
    async def scenario(app, pilot, runner):
        await pilot.press("slash")
        await type_text(pilot, "shop")
        await pilot.press("enter")
        await pilot.pause()
        await goto(app, pilot, ("project", "/home/x/shop"))
        await pilot.press("enter")  # close it
        await pilot.press("r")
        await settle(app, pilot)
        assert not labels(app)[0].endswith("[open]")
    run(scenario, tmp_path)


def test_after_a_delete_the_cursor_goes_to_the_next_row_in_the_project(tmp_path):
    data = sessions(tmp_path)

    class Deleting(FakeRunner):
        def delete(self, s, job):
            super().delete(s, job)
            data[:] = [x for x in data if x.session_id != s.session_id]

    async def scenario(app, pilot, runner):
        await goto(app, pilot, ("session", "run-1"))  # media: run-1, then stop-1
        await pilot.press("x")
        await pilot.pause()
        await pilot.press("y")
        await settle(app, pilot)
        assert runner.calls == [("rm", "run1")]
        assert cursor(app) == ("session", "stop-1")
        await pilot.press("x")
        await pilot.pause()
        await pilot.press("y")
        await settle(app, pilot)
        # media's last session is gone, so is the project: the cursor falls back to the first row
        assert cursor(app) == ("project", "/opt/plex")
    run(scenario, tmp_path, data=data, runner=Deleting())


def test_titles_with_brackets_show_as_typed(tmp_path):
    data = sessions(tmp_path)
    data[0] = Session("job-1", "fix [bold] and [/] things", "/opt/plex", "/opt/plex", "needs you", NOW - 60,
                      job="job1")

    async def scenario(app, pilot, runner):
        await goto(app, pilot, ("session", "job-1"))
        await pilot.press("x")
        await pilot.pause()
        assert "fix [bold] and [/] things" in str(app.screen.query_one("Label").render())
        await pilot.press("y")
        await settle(app, pilot)
        assert "Deleted “fix [bold] and [/] things”." in notices(app)
    run(scenario, tmp_path, data=data)


def test_a_delete_still_lands_when_the_board_cant_write_its_list(tmp_path, monkeypatch):
    from board import app as app_module
    calls = []
    monkeypatch.setattr(app_module.actions, "delete_background", lambda job: calls.append(job))

    def broken(*a, **kw):
        raise OSError("read-only file system")
    monkeypatch.setattr(app_module.facts, "forget", broken)
    s = Session("a", "t", "/", "/", "idle", NOW, job="j")
    app_module.Runner().delete(s, "j")
    assert calls == ["j"]


# --- e: ended sessions -------------------------------------------------------------------------------

def test_ended_sessions_are_hidden_until_e(tmp_path):
    async def scenario(app, pilot, runner):
        rows = "\n".join(labels(app))
        assert "old chat" not in rows and not any(r.startswith("shop") for r in labels(app))
        assert "3 ended hidden (e)" in app.sub_title
        await pilot.press("e")
        await pilot.pause()
        assert "old chat" in "\n".join(labels(app)) and "hidden" not in app.sub_title
        await pilot.press("e")
        await pilot.pause()
        assert "old chat" not in "\n".join(labels(app))
    run(scenario, tmp_path, show_ended=False)


def test_the_filter_finds_ended_sessions_even_when_hidden(tmp_path):
    async def scenario(app, pilot, runner):
        await pilot.press("slash")
        await type_text(pilot, "shop")
        await pilot.pause()
        assert any("shop question" in r for r in labels(app))
        await pilot.press("escape")
        await pilot.pause()
        assert not any("shop question" in r for r in labels(app))
    run(scenario, tmp_path, show_ended=False)


def test_the_choice_is_remembered(tmp_path):
    from board import facts
    prefs = str(tmp_path / "state" / "prefs.json")

    async def go():
        app = Board(source=lambda: sessions(tmp_path), runner=FakeRunner(), poll=None, prefs_path=prefs)
        async with app.run_test(size=(120, 40)) as pilot:
            await app.workers.wait_for_complete()
            await pilot.press("e")
            await pilot.pause()
    asyncio.run(go())
    assert facts.load_prefs(prefs) == {"show_ended": True}


# --- waiting on, stale, and the columns ----------------------------------------------------------------

from board.app import detail_lines, model_name, tokens  # noqa: E402


def test_a_row_that_needs_you_says_what_for():
    s = Session("a", "Home assistant audit", "/opt/ha", "/opt/ha", "needs you", NOW - 60, job="j",
                waiting="Bash: docker compose restart homeassistant")
    label = str(session_label(s, 140))
    assert "Home assistant audit — Bash: docker compose restart homeassistant" in label
    assert "Needs: Bash: docker compose restart homeassistant" in "\n".join(str(t) for t in detail_lines(s, 100))


def test_waiting_is_cut_to_fit_and_never_pushes_the_state_off():
    s = Session("a", "audit", "/opt", "/opt", "needs you", NOW - 60, job="j", waiting="x" * 300)
    label = str(session_label(s, 100, show_columns=False))
    assert len(label) <= 100 and "needs you · now" in label and "…" in label


def test_waiting_shows_only_when_it_needs_you():
    s = Session("a", "audit", "/opt", "/opt", "running", NOW, job="j", waiting="old question")
    assert "old question" not in str(session_label(s, 140))


def test_a_stale_job_says_not_running_and_why():
    s = Session("a", "Home assistant audit", "/opt/ha", "/opt/ha", "stale", NOW - 86400 * 5, job="j",
                waiting="rate limited — wait and retry", was="needs you")
    label = str(session_label(s, 140))
    assert label.startswith("◇ Home assistant audit — rate limited") and "not running · 5d" in label
    text = "\n".join(str(t) for t in detail_lines(s, 100))
    assert "Needed: rate limited" in text and "“it needs you” is out of date" in text


def test_stale_jobs_are_counted_apart(tmp_path):
    data = sessions(tmp_path) + [Session("st-1", "gone quiet", "/opt/plex", "/opt/plex", "stale", NOW - 99,
                                         job="st1", was="running")]

    async def scenario(app, pilot, runner):
        assert "1 need you · 1 running · 1 not running" in app.sub_title
        await goto(app, pilot, ("session", "st-1"))
        await pilot.press("enter")
        await pilot.pause()
        assert runner.calls == [("attach", "st1")]
    run(scenario, tmp_path, data=data)


def test_columns_show_context_model_and_tokens():
    s = Session("a", "plex", "/opt", "/opt", "idle", NOW, job="j", model="claude-opus-5-5",
                context=182_005, output=310_400)
    assert "182k ctx  Opus 5.5    310k out" in str(session_label(s, 140))
    assert "ctx" not in str(session_label(s, 90))                       # too narrow
    assert "ctx" not in str(session_label(s, 140, show_columns=False))  # hidden with c
    assert "Model:  Opus 5.5 · 182k in context · 310k tokens written" in \
        "\n".join(str(t) for t in detail_lines(s, 100))


def test_rows_line_up():
    a = Session("a", "short", "/opt", "/opt", "idle", NOW, job="j", model="claude-opus-5-5", context=9, output=9)
    b = Session("b", "a much longer title here", "/opt", "/opt", "needs you", NOW, job="k",
                model="claude-sonnet-4-5-20250929", context=1_200_000, output=0, waiting="Which repo?")
    la, lb = str(session_label(a, 140)), str(session_label(b, 140))
    assert la.index(" ctx") == lb.index(" ctx") and len(la) == len(lb)


def test_model_names_and_token_counts():
    assert model_name("claude-opus-5-5") == "Opus 5.5"
    assert model_name("claude-sonnet-4-5-20250929") == "Sonnet 4.5"
    assert model_name("claude-fable-5-1") == "Fable 5.1"
    assert model_name(None) == ""
    assert [tokens(n) for n in (850, 182_005, 1_250_000)] == ["850", "182k", "1.2M"]


def test_c_hides_the_columns_and_is_remembered(tmp_path):
    from board import facts
    prefs = str(tmp_path / "state" / "prefs.json")
    data = [Session("a", "plex", "/opt/plex", "/opt/plex", "idle", NOW, job="j", model="claude-opus-5-5",
                    context=5000, output=10)]

    async def go():
        app = Board(source=lambda: list(data), runner=FakeRunner(), poll=None, prefs_path=prefs)
        async with app.run_test(size=(140, 40)) as pilot:
            await app.workers.wait_for_complete()
            await pilot.pause()
            assert any("5k ctx" in r for r in labels(app))
            await pilot.press("c")
            await pilot.pause()
            assert not any("ctx" in r for r in labels(app))
    asyncio.run(go())
    assert facts.load_prefs(prefs) == {"show_columns": False}


def test_status_that_repeats_the_need_is_not_shown_twice():
    s = Session("a", "audit", "/opt", "/opt", "needs you", NOW, job="j", summary="limit hit ·  resets 5am",
                waiting="rate limited — wait and retry · limit hit · resets 5am")
    text = "\n".join(str(t) for t in detail_lines(s, 200))
    assert "Needs: rate limited" in text and "Status:" not in text
