import json
import sqlite3
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import chat_read_later as script


ACCOUNT = "test@example.com"
DM = "spaces/DM"
ROOM = "spaces/Room"
THREAD = f"{ROOM}/threads/T"
BEFORE = "2026-11-20T11:59:00Z"
FIRST = "2026-11-20T12:00:00.000000001Z"
SECOND = "2026-11-20T12:01:00Z"


def message(target, identifier, created):
    space, _, thread = target.partition("/threads/")
    row = {"name": f"{space}/messages/{identifier}", "createTime": created}
    if thread:
        row.update(threadReply=True, thread={"name": target})
    return row


class RestoreProtectionTest(unittest.TestCase):
    def setUp(self):
        self.args = SimpleNamespace(account=ACCOUNT)
        self.db = sqlite3.connect(":memory:")
        self.addCleanup(self.db.close)
        self.cursors = {DM: BEFORE, ROOM: BEFORE, THREAD: BEFORE}
        self.messages = {DM: [], ROOM: [], THREAD: []}
        self.hidden_from_search = set()
        self.patches = []
        for name, replacement in (
            ("read_state", self.read_state),
            ("unread_messages", self.unread_messages),
        ):
            mock = patch.object(script, name, replacement)
            mock.start()
            self.addCleanup(mock.stop)

    def read_state(self, args, target, cursor=None):
        if cursor is not None:
            self.patches.append((target, cursor))
            self.cursors[target] = cursor
        return self.cursors[target]

    def unread_messages(self, args):
        return {
            target: {
                msg["name"]: msg for msg in messages
                if msg["name"] not in self.hidden_from_search
                and script.cursor_key(msg["createTime"]) > script.cursor_key(self.cursors[target])
            }
            for target, messages in self.messages.items()
        }

    def test_restored_messages_hold_their_read_state_until_read(self):
        script.ensure_schema(self.db)
        original_dm = message(DM, "D1", FIRST)
        original_reply = message(THREAD, "T1", FIRST)
        self.messages[DM] = [original_dm]
        self.messages[THREAD] = [original_reply]
        self.assertEqual(script.stash(self.args, self.db)[0]["updated"], 2)
        self.assertEqual(script.restore(self.args, self.db)[0]["restored"], 2)

        # The search index can omit restored messages while showing newer arrivals.
        self.hidden_from_search = {original_dm["name"], original_reply["name"]}
        self.messages[DM].append(message(DM, "D2", SECOND))
        self.messages[THREAD].append(message(THREAD, "T2", SECOND))
        self.messages[ROOM].append(message(ROOM, "R1", SECOND))
        self.patches.clear()

        result, code = script.stash(self.args, self.db)
        self.assertEqual(code, 0)
        self.assertEqual(result, {"saved": 3, "updated": 1, "deferredTargets": 2, "failures": []})
        self.assertEqual(self.patches, [(ROOM, SECOND)])
        self.assertEqual(self.cursors[DM], BEFORE)
        self.assertEqual(self.cursors[THREAD], BEFORE)
        self.assertEqual(script.stash(self.args, self.db)[0]["updated"], 0)

        self.cursors[DM] = FIRST
        self.cursors[THREAD] = FIRST
        self.patches.clear()
        self.assertEqual(script.stash(self.args, self.db)[0]["updated"], 2)
        self.assertCountEqual(self.patches, [(DM, SECOND), (THREAD, SECOND)])
        self.assertEqual(script.restore(self.args, self.db)[0]["restored"], 3)
        self.assertEqual(self.cursors[DM], FIRST)
        self.assertEqual(self.cursors[THREAD], FIRST)
        self.assertEqual(script.stash(self.args, self.db)[0]["updated"], 0)

    def test_upgrade_preserves_restored_and_pending_messages(self):
        self.db.executescript("""
            CREATE TABLE pending (
                account TEXT NOT NULL,
                target TEXT NOT NULL,
                original_read_time TEXT NOT NULL,
                PRIMARY KEY (account, target)
            );
        """)
        self.db.executescript(script.SCHEMA)
        with self.db:
            self.db.execute("INSERT INTO pending VALUES (?, ?, ?)", (ACCOUNT, THREAD, BEFORE))
            self.db.executemany(
                "INSERT INTO saved_messages VALUES (?, ?, ?, ?, ?)",
                [
                    (ACCOUNT, f"{DM}/messages/D1", DM, 1, json.dumps(message(DM, "D1", FIRST))),
                    (
                        ACCOUNT, f"{ROOM}/messages/T1", THREAD, 1,
                        json.dumps(message(THREAD, "T1", SECOND)),
                    ),
                ],
            )

        script.ensure_schema(self.db)
        script.ensure_schema(self.db)
        self.assertEqual(script.restored_through(self.db, ACCOUNT, DM), FIRST)
        self.assertEqual(self.db.execute("SELECT marked_through FROM pending").fetchone(), (SECOND,))
        self.cursors[THREAD] = SECOND
        self.assertEqual(script.restore(self.args, self.db)[0]["restored"], 1)
        self.assertEqual(script.restored_through(self.db, ACCOUNT, THREAD), SECOND)
        self.assertEqual(self.cursors[THREAD], BEFORE)
