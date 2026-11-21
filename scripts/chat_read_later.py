#!/usr/bin/env python3
"""Save Google Chat unread messages locally and restore their unread status later."""

import argparse
from contextlib import closing
from datetime import datetime, timezone
import fcntl
import json
import os
from pathlib import Path
import re
import sqlite3
import subprocess
import sys
import time


DEFAULT_BINARY = Path(__file__).resolve().parents[1] / "target/release/gchat"
DEFAULT_DB = Path.home() / ".config/gchat/read-later.sqlite3"
EPOCH = "1970-01-01T00:00:00Z"
BOT_SCAN_OVERLAP_SECONDS = 60
SCHEMA = """
CREATE TABLE IF NOT EXISTS saved_messages (
    account TEXT NOT NULL,
    name TEXT NOT NULL,
    target TEXT NOT NULL,
    saved_at INTEGER NOT NULL,
    message_json TEXT NOT NULL,
    PRIMARY KEY (account, name)
);
CREATE TABLE IF NOT EXISTS pending (
    account TEXT NOT NULL,
    target TEXT NOT NULL,
    original_read_time TEXT NOT NULL,
    marked_through TEXT NOT NULL,
    PRIMARY KEY (account, target)
);
CREATE TABLE IF NOT EXISTS restored (
    account TEXT NOT NULL,
    target TEXT NOT NULL,
    through_time TEXT NOT NULL,
    PRIMARY KEY (account, target)
);
CREATE TABLE IF NOT EXISTS bot_scans (
    account TEXT NOT NULL,
    space TEXT NOT NULL,
    sender TEXT NOT NULL,
    scanned_at INTEGER NOT NULL,
    PRIMARY KEY (account, space, sender)
);
"""


def cursor_key(value):
    match = re.fullmatch(r"(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2})(?:\.(\d{1,9}))?(Z|[+-]\d{2}:\d{2})", value)
    if not match:
        raise ValueError(f"Invalid Google Chat timestamp {value}")
    seconds, fraction, offset = match.groups()
    whole = datetime.fromisoformat(seconds + offset.replace("Z", "+00:00"))
    return whole, (fraction or "").ljust(9, "0")


def chat(args, *command):
    argv = [str(args.gchat), "--no-display-names"]
    if args.account:
        argv.extend(["--account", args.account])
    result = subprocess.run(
        [*argv, *command], capture_output=True, text=True, timeout=45,
    )
    try:
        response = json.loads(result.stdout or result.stderr)
    except ValueError as error:
        raise RuntimeError(f"gchat produced no usable response (exit {result.returncode})") from error
    if result.returncode or not response.get("ok"):
        raise RuntimeError(response.get("error", {}).get("message", "gchat command failed"))
    return response


def select_account(args):
    if args.account:
        return args.account.strip().lower()
    response = chat(args, "auth", "list")
    if default := response["meta"].get("defaultAccount"):
        return default
    accounts = response["data"]["accounts"]
    if len(accounts) == 1:
        return accounts[0]["email"]
    raise RuntimeError("Select a configured gchat account with --account")


def read_state(args, target, cursor=None):
    space, _, thread = target.partition("/threads/")
    command = ["chat", "read-state", space]
    if thread:
        command.extend(["--thread", thread])
    if cursor is not None:
        command.extend(["--last-read-time", cursor])
    return chat(args, *command)["data"].get("lastReadTime", EPOCH)


def group_message(groups, message):
    name = message["name"]
    space, separator, _ = name.partition("/messages/")
    if not separator or not space.startswith("spaces/"):
        raise ValueError(f"Invalid Chat message name {name}")
    target = message["thread"]["name"] if message.get("threadReply") else space
    if target.startswith("threads/"):
        target = f"{space}/{target}"
    if message.get("threadReply") and not target.startswith(f"{space}/threads/"):
        raise ValueError(f"Invalid thread name for {name}")
    groups.setdefault(target, {})[name] = message


def unread_messages(args):
    response = chat(args, "search", "unread", "--view", "full", "--all", "--include-marked")
    if response["meta"].get("truncated"):
        raise RuntimeError("Google Chat unread search returned an incomplete page set")
    groups = {}
    for result in response["data"]["results"]:
        if result.get("read") is False:
            group_message(groups, result["message"])
    return groups


def scan_bot_sources(args, db, groups, failures):
    scans = []
    for space, sender, since in args.bot_source:
        try:
            scanned_at = int(time.time())
            previous = db.execute(
                "SELECT scanned_at FROM bot_scans WHERE account = ? AND space = ? AND sender = ?",
                (args.account, space, sender),
            ).fetchone() if db is not None else None
            if previous:
                overlap = datetime.fromtimestamp(
                    previous[0] - BOT_SCAN_OVERLAP_SECONDS, timezone.utc,
                ).isoformat()
                since = max(since, overlap, key=cursor_key)
            page = chat(args, "chat", "messages", space, "--after", since, "--all")
            if page["meta"].get("truncated") or page["meta"].get("nextPageToken"):
                raise RuntimeError("Google Chat space listing returned an incomplete page set")
            listed = {}
            for message in page["data"]["messages"]:
                author = message.get("sender") or {}
                if author.get("type") == "BOT" and author.get("name") == sender:
                    group_message(listed, message)
            for target, messages in listed.items():
                groups.setdefault(target, {}).update(messages)
            scans.append((space, sender, scanned_at, set(listed)))
        except (RuntimeError, ValueError, KeyError, OSError, subprocess.TimeoutExpired) as error:
            failures.append({"source": space, "error": str(error)})
    return scans


def restored_through(db, account, target):
    row = db.execute(
        "SELECT through_time FROM restored WHERE account = ? AND target = ?", (account, target),
    ).fetchone()
    return row[0] if row else None


def protect_restored(db, account, target, through_time):
    previous = restored_through(db, account, target)
    if previous is None or cursor_key(through_time) > cursor_key(previous):
        db.execute(
            """INSERT INTO restored VALUES (?, ?, ?)
               ON CONFLICT(account, target) DO UPDATE SET through_time=excluded.through_time""",
            (account, target, through_time),
        )


def save_messages(db, account, target, messages):
    now = int(time.time())
    return db.executemany(
        """INSERT INTO saved_messages VALUES (?, ?, ?, ?, ?)
           ON CONFLICT(account, name) DO NOTHING""",
        ((account, msg["name"], target, now, json.dumps(msg)) for msg in messages),
    ).rowcount


def stash(args, db):
    groups = unread_messages(args)
    updated = 0
    saved = 0
    deferred = 0
    failures = []
    scans = scan_bot_sources(args, db, groups, failures)
    unfinished_targets = set()
    for target in sorted(groups):
        messages = groups[target]
        try:
            current = read_state(args, target)
            current_key = cursor_key(current)
            unread = [
                message for message in messages.values()
                if cursor_key(message["createTime"]) > current_key
            ]
            if not unread:
                continue
            protected = restored_through(db, args.account, target)
            if protected is not None and current_key < cursor_key(protected):
                # Advancing this read timestamp would clear the restored messages too.
                with db:
                    saved += save_messages(db, args.account, target, unread)
                deferred += 1
                unfinished_targets.add(target)
                continue

            latest = max((message["createTime"] for message in unread), key=cursor_key)
            pending = db.execute(
                "SELECT marked_through FROM pending WHERE account = ? AND target = ?",
                (args.account, target),
            ).fetchone()
            marked_through = max(latest, pending[0], key=cursor_key) if pending and pending[0] else latest
            with db:
                db.execute(
                    """INSERT INTO pending (account, target, original_read_time, marked_through)
                       VALUES (?, ?, ?, ?)
                       ON CONFLICT(account, target) DO UPDATE SET marked_through=excluded.marked_through""",
                    (args.account, target, current, marked_through),
                )
                saved += save_messages(db, args.account, target, unread)
            marked = read_state(args, target, latest)
            if cursor_key(marked) < cursor_key(latest):
                raise RuntimeError("Google returned a read timestamp earlier than the saved messages")
            updated += 1
        except (RuntimeError, ValueError, OSError, subprocess.TimeoutExpired) as error:
            failures.append({"target": target, "error": str(error)})
            unfinished_targets.add(target)
    # Keep listing any range with unread bot messages that still need a write.
    with db:
        db.executemany(
            """INSERT INTO bot_scans VALUES (?, ?, ?, ?)
               ON CONFLICT(account, space, sender) DO UPDATE
               SET scanned_at=MAX(bot_scans.scanned_at, excluded.scanned_at)""",
            (
                (args.account, space, sender, scanned_at)
                for space, sender, scanned_at, targets in scans
                if unfinished_targets.isdisjoint(targets)
            ),
        )
    return {
        "saved": saved, "updated": updated, "deferredTargets": deferred, "failures": failures,
    }, int(bool(failures))


def restore(args, db):
    targets = db.execute(
        """SELECT target, original_read_time, marked_through
           FROM pending WHERE account = ? ORDER BY target""",
        (args.account,),
    ).fetchall()
    restored = 0
    failures = []
    for target, original, marked_through in targets:
        try:
            current = read_state(args, target)
            if marked_through is not None:
                # Persist protection before changing Google so a retry keeps it in place.
                with db:
                    protect_restored(db, args.account, target, marked_through)
            original_key = cursor_key(original)
            if cursor_key(current) > original_key:
                current = read_state(args, target, original)
                if cursor_key(current) > original_key:
                    raise RuntimeError("Google returned a read timestamp later than the saved timestamp")
            with db:
                db.execute("DELETE FROM pending WHERE account = ? AND target = ?", (args.account, target))
            restored += 1
        except (RuntimeError, ValueError, OSError, subprocess.TimeoutExpired) as error:
            failures.append({"target": target, "error": str(error)})
    return {"restored": restored, "failures": failures}, int(bool(failures))


def ensure_schema(db):
    db.executescript(SCHEMA)
    if "marked_through" in {column[1] for column in db.execute("PRAGMA table_info(pending)")}:
        return
    # Older databases kept snapshots after restore and removed the corresponding pending row.
    with db:
        db.execute("BEGIN")
        db.execute("ALTER TABLE pending ADD COLUMN marked_through TEXT")
        boundaries = {}
        for account, target, message_json, pending in db.execute(
            """SELECT s.account, s.target, s.message_json, p.target IS NOT NULL
               FROM saved_messages s
               LEFT JOIN pending p ON p.account = s.account AND p.target = s.target"""
        ):
            created = json.loads(message_json)["createTime"]
            key = (account, target, pending)
            boundaries[key] = max(boundaries.get(key, created), created, key=cursor_key)
        for (account, target, pending), through_time in boundaries.items():
            if pending:
                db.execute(
                    "UPDATE pending SET marked_through = ? WHERE account = ? AND target = ?",
                    (through_time, account, target),
                )
            else:
                protect_restored(db, account, target, through_time)


def status(args):
    if not args.db.exists():
        return {"database": str(args.db), "savedMessages": 0, "pendingTargets": 0}
    with closing(sqlite3.connect(f"{args.db.resolve().as_uri()}?mode=ro", uri=True)) as db:
        (saved,) = db.execute(
            "SELECT COUNT(*) FROM saved_messages WHERE account = ?", (args.account,),
        ).fetchone()
        (pending,) = db.execute(
            "SELECT COUNT(*) FROM pending WHERE account = ?", (args.account,),
        ).fetchone()
        return {"database": str(args.db), "savedMessages": saved, "pendingTargets": pending}


def run_once(args, operation):
    args.db.parent.mkdir(parents=True, exist_ok=True)
    with args.db.with_name(args.db.name + ".lock").open("a") as lock:
        mode = fcntl.LOCK_EX | fcntl.LOCK_NB if operation == stash else fcntl.LOCK_EX
        try:
            fcntl.flock(lock, mode)
        except BlockingIOError:
            return {"skipped": "another run is active"}, 0
        with closing(sqlite3.connect(args.db)) as db:
            ensure_schema(db)
            return operation(args, db)


def main():
    os.umask(0o077)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--account", help="Use this account instead of the gchat default")
    parser.add_argument("--gchat", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--db", type=Path, default=DEFAULT_DB)
    parser.add_argument(
        "--bot-source", action="append", nargs=3, default=[], metavar=("SPACE", "SENDER", "AFTER"),
        help="Also list messages by this bot in this space after an RFC 3339 timestamp",
    )
    commands = parser.add_subparsers(dest="command", required=True)
    stash_parser = commands.add_parser("stash", help="Save unread messages and then mark them read")
    stash_parser.add_argument("--dry-run", action="store_true", help="Count matching messages")
    commands.add_parser("watch", help="Run stash once per minute until stopped")
    commands.add_parser("restore", help="Restore saved unread states until you read them")
    commands.add_parser("status", help="Show saved message count and pending restores")
    args = parser.parse_args()
    for space, sender, since in args.bot_source:
        if not re.fullmatch(r"spaces/[\w-]+", space) or not re.fullmatch(r"users/[\w-]+", sender):
            parser.error("--bot-source requires spaces/ID, users/ID, and a timestamp")
        cursor_key(since)
    args.account = select_account(args)

    if args.command == "status":
        print(json.dumps(status(args)))
    elif args.command == "stash" and args.dry_run:
        groups = unread_messages(args)
        failures = []
        scans = scan_bot_sources(args, None, groups, failures)
        if failures:
            raise RuntimeError("; ".join(failure["error"] for failure in failures))
        bot_targets = {target for _, _, _, targets in scans for target in targets}
        for target in bot_targets:
            current = cursor_key(read_state(args, target))
            groups[target] = {
                name: message for name, message in groups[target].items()
                if cursor_key(message["createTime"]) > current
            }
            if not groups[target]:
                del groups[target]
        print(json.dumps({"unreadMessages": sum(map(len, groups.values())), "targets": len(groups)}))
    elif args.command == "watch":
        while True:
            began_at = time.monotonic()
            try:
                result, _ = run_once(args, stash)
                print(json.dumps(result), flush=True)
            except (RuntimeError, ValueError, KeyError, OSError, sqlite3.Error, subprocess.TimeoutExpired) as error:
                print(f"Chat read-later: {error}", file=sys.stderr, flush=True)
            time.sleep(max(0, 60 - (time.monotonic() - began_at)))
    else:
        result, exit_code = run_once(args, restore if args.command == "restore" else stash)
        print(json.dumps(result))
        return exit_code
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        sys.exit(130)
    except (RuntimeError, ValueError, KeyError, OSError, sqlite3.Error, subprocess.TimeoutExpired) as error:
        sys.exit(f"Chat read-later: {error}")
