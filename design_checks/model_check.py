"""Executable design model, not a production codec, DB, or network implementation.

Run: python3 design_checks/model_check.py
Uses one Content; transport queues are NOT accepted/acknowledged storage.
"""

from copy import deepcopy
from dataclasses import dataclass, replace
from itertools import permutations


ABSENT = ("absent",)


@dataclass(frozen=True)
class Change:
    key: str
    value: object
    parents: tuple = ()


@dataclass(frozen=True)
class Op:
    id: str
    action: str
    basis: tuple = ()
    changes: tuple = ()
    time: int = 0


def live(op):
    return op.action != "delete"


def closure(ops, basis):
    found = set()
    todo = list(basis)
    while todo:
        key = todo.pop()
        if key not in found:
            found.add(key)
            todo.extend(ops[key].basis)
    return found


def life_parents(ops, op):
    return {key for key in op.basis if op.action != "edit" or live(ops[key])}


def oracle(ops):
    """Set-based definition, independent of incremental head maintenance."""
    life = set(ops)
    fields = {}
    removed = {}
    for op in ops.values():
        life -= life_parents(ops, op)
        for change in op.changes:
            fields.setdefault(change.key, set()).add(op.id)
            removed.setdefault(change.key, set()).update(change.parents)
    return life, {key: heads - removed[key] for key, heads in fields.items()}


def values(ops, field_heads):
    result = {}
    for key, heads in field_heads.items():
        result[key] = {
            next(c.value for c in ops[head].changes if c.key == key)
            for head in heads
        }
    return result


def restore_changes(ops, basis, supplied):
    scope = {key: ops[key] for key in closure(ops, basis)}
    _, fields = oracle(scope)
    assert "body" in supplied
    return tuple(
        Change(key, supplied.get(key, False if key.startswith("kind:") else ABSENT),
               tuple(sorted(fields.get(key, ()))))
        for key in sorted(set(fields) | set(supplied))
    )


class Model:
    def __init__(self):
        self.ops = {}
        self.life = set()
        self.fields = {}
        self.log = []

    def accept(self, op):
        if op.id in self.ops:
            if self.ops[op.id] != op:
                raise ValueError("ID mismatch")
            return False
        if any(key not in self.ops for key in op.basis):
            raise ValueError("missing dependency")
        if op.id in op.basis or len(set(op.basis)) != len(op.basis):
            raise ValueError("invalid basis")
        scope = closure(self.ops, op.basis)
        if op.action == "create":
            if self.ops or op.basis or not any(c.key == "body" for c in op.changes):
                raise ValueError("invalid root")
        elif not op.basis:
            raise ValueError("missing basis")
        scoped = {key: self.ops[key] for key in scope}
        if oracle(scoped)[0] != set(op.basis):
            raise ValueError("basis is not a frontier")
        signs = {live(self.ops[key]) for key in op.basis}
        if op.action == "edit" and True not in signs:
            raise ValueError("edit cannot restore")
        if op.action == "keep" and signs != {True, False}:
            raise ValueError("keep needs a conflict")
        if op.action == "restore" and signs != {False}:
            raise ValueError("restore needs deleted basis")
        if op.action not in {"create", "edit", "delete", "keep", "restore"}:
            raise ValueError("unknown action")
        if op.action in {"delete", "keep"} and op.changes:
            raise ValueError("unexpected changes")
        if len({c.key for c in op.changes}) != len(op.changes):
            raise ValueError("duplicate field")
        for change in op.changes:
            for parent in change.parents:
                if parent not in scope or not any(
                    c.key == change.key for c in self.ops[parent].changes
                ):
                    raise ValueError("invalid field parent")
        if op.action == "restore":
            supplied = {c.key: c.value for c in op.changes}
            if op.changes != restore_changes(self.ops, op.basis, supplied):
                raise ValueError("incomplete restore")
        self.life -= life_parents(self.ops, op)
        self.life.add(op.id)
        for change in op.changes:
            self.fields.setdefault(change.key, set()).difference_update(change.parents)
            self.fields[change.key].add(op.id)
        self.ops[op.id] = op
        self.log.append(op.id)
        assert (self.life, self.fields) == oracle(self.ops)
        return True

    def normal(self, key):
        head = max(self.fields[key], key=lambda h: (self.ops[h].time, h))
        return next(c.value for c in self.ops[head].changes if c.key == key)


def deliver(order):
    model = Model()
    pending = []
    for op in order:
        pending.append(op)
        while True:
            ready = next((o for o in pending if all(p in model.ops for p in o.basis)), None)
            if ready is None:
                break
            pending.remove(ready)
            model.accept(ready)
            model.accept(ready)  # duplicated packet / lost ACK
    assert not pending
    return model


def scenario(name, ops, life, field_values, normal=None):
    count = 0
    for order in permutations(ops):
        model = deliver(order)
        assert model.life == set(life), (name, model.life)
        assert values(model.ops, model.fields) == field_values, name
        assert len(model.log) == len(ops), name
        for key, value in (normal or {}).items():
            assert model.normal(key) == value, name
        count += 1
    print(f"{name}: {count} arrival permutations passed")
    return count


def expect_error(call):
    try:
        call()
    except ValueError:
        return
    raise AssertionError("expected rejection")


class Replica:
    """Copy-on-commit transaction model for one incoming log stream."""
    def __init__(self):
        self.model = Model()
        self.cursors = {}

    def receive(self, epoch, after, through, high, entries, fail_before_commit=False):
        cursor = self.cursors.get(epoch, 0)
        if cursor != after:
            return cursor  # caller resynchronizes its position
        if not after <= through <= high:
            raise ValueError("range")
        seqs = [seq for seq, _ in entries]
        if seqs != sorted(set(seqs)) or any(not after < s <= through for s in seqs):
            raise ValueError("sequence")
        if (seqs and through != seqs[-1]) or (not seqs and through != high):
            raise ValueError("page boundary")
        staged = deepcopy(self.model)
        for _, op in entries:
            staged.accept(op)
        if fail_before_commit:
            raise ValueError("simulated crash")
        self.model = staged
        self.cursors[epoch] = through
        return through


def check_transport(root, edit):
    replica = Replica()
    page = [(1, root), (3, edit)]  # legitimate sequence gap
    expect_error(lambda: replica.receive("e1", 0, 3, 3, page, True))
    assert not replica.model.ops and not replica.cursors
    assert replica.receive("e1", 0, 3, 3, page) == 3
    assert replica.receive("e1", 0, 3, 3, page) == 3  # ACK lost
    assert len(replica.model.log) == 2
    # New numbering / full rescan: duplicates do not create new local log rows.
    assert replica.receive("e2", 0, 2, 2, [(1, root), (2, edit)]) == 2
    assert len(replica.model.log) == 2
    # A valid entry followed by an invalid one must not partially commit.
    tail = Op("tail", "edit", (edit.id,), (Change("body", "tail", (edit.id,)),))
    bad = Op("bad", "edit", ("missing",))
    before = deepcopy(replica.model.ops)
    expect_error(lambda: replica.receive("e2", 2, 4, 4, [(3, tail), (4, bad)]))
    assert replica.model.ops == before and replica.cursors["e2"] == 2
    expect_error(lambda: replica.model.accept(replace(edit, time=999)))
    expect_error(lambda: Replica().receive("e", 0, 1, 1, [(1, edit)]))
    print("transport: rollback, ACK loss, epoch rescan, gaps, bad dependencies and ID mismatch passed")


def check_relay(root, a, b, b2):
    peers = {name: Replica() for name in ("A", "S", "B")}
    peers["A"].model = deliver([root, a])
    peers["B"].model = deliver([root, b, b2])

    def transfer(sender, receiver):
        source, target = peers[sender], peers[receiver]
        epoch = sender + ":epoch1"
        high = len(source.model.log)
        while target.cursors.get(epoch, 0) < high:
            after = target.cursors.get(epoch, 0)
            through = min(after + 2, high)
            page = [(i + 1, source.model.ops[source.model.log[i]]) for i in range(after, through)]
            target.receive(epoch, after, through, high, page)

    transfer("A", "S")
    assert "b1" not in peers["S"].model.ops  # B still partitioned
    for _ in range(8):
        before = sum(len(p.model.log) for p in peers.values())
        for sender, receiver in (("B", "S"), ("S", "A"), ("A", "S"), ("S", "B")):
            transfer(sender, receiver)
        if sum(len(p.model.log) for p in peers.values()) == before:
            break
    else:
        raise AssertionError("relay did not settle")
    for peer in peers.values():
        assert set(peer.model.ops) == {"v0", "a1", "b1", "b2"}
        assert peer.model.life == {"a1", "b2"}
        assert len(peer.model.log) == 4  # echoes do not produce log growth
    print("relay: partition, A-S-B forwarding, bidirectional catch-up and finite echoes passed")


def main():
    root = Op("v0", "create", changes=(Change("body", "base"),))
    a = Op("a1", "edit", ("v0",), (Change("body", "A", ("v0",)),), 10)
    b = Op("b1", "edit", ("v0",), (Change("body", "B", ("v0",)),), 20)
    b2 = Op("b2", "edit", ("b1",), (Change("body", "B2", ("b1",)),), 30)
    r = Op("r1", "edit", ("a1", "b1"), (Change("body", "R", ("a1", "b1")),), 40)
    d = Op("d1", "delete", ("v0",))
    total = scenario("resolution versus late edit", [root, a, b, b2, r],
                     {"b2", "r1"}, {"body": {"B2", "R"}})
    title = Op("t", "edit", ("a1", "b1"), (Change("title", "title"),))
    total += scenario("title edit retains body conflict", [root, a, b, title],
                      {"t"}, {"body": {"A", "B"}, "title": {"title"}})
    during = Op("e", "edit", ("a1", "d1"), (Change("body", "E", ("a1",)),))
    total += scenario("edit does not silently keep", [root, a, d, during],
                      {"e", "d1"}, {"body": {"E"}})
    keep = Op("k", "keep", ("a1", "d1"))
    delete = Op("d2", "delete", ("a1", "d1"))
    total += scenario("keep versus delete", [root, a, d, keep, delete],
                      {"k", "d2"}, {"body": {"A"}})
    old = Op("old", "edit", ("a1",), (Change("body", "newer causally", ("a1",)),), -100)
    total += scenario("clock reversal", [root, a, old], {"old"},
                      {"body": {"newer causally"}}, {"body": "newer causally"})
    ka = Op("ka", "edit", ("v0",), (Change("kind:wiki", True),))
    kb = Op("kb", "edit", ("v0",), (Change("kind:wiki", True),))
    kd = Op("kd", "edit", ("ka", "kb"), (Change("kind:wiki", False, ("ka", "kb")),))
    total += scenario("same-value kind adds then remove", [root, ka, kb, kd],
                      {"kd"}, {"body": {"base"}, "kind:wiki": {False}})
    kr = Op("kr", "edit", ("ka", "kb"), (Change("kind:wiki", True, ("ka", "kb")),))
    total += scenario("kind removal versus explicit readd", [root, ka, kb, kd, kr],
                      {"kd", "kr"}, {"body": {"base"}, "kind:wiki": {False, True}})
    rich = Op("v0", "create", changes=(Change("body", "base"), Change("secret", "old")))
    known = {"v0": rich, "d1": d}
    restore = Op("r", "restore", ("d1",), restore_changes(known, ("d1",), {"body": "restored"}))
    unseen = Op("u", "edit", ("v0",), (Change("unseen", "added"), Change("secret", "new", ("v0",))))
    total += scenario("restore clears known but preserves concurrent fields", [rich, d, restore, unseen],
                      {"r", "u"}, {"body": {"restored"}, "secret": {ABSENT, "new"}, "unseen": {"added"}})
    restore2 = Op("r2", "restore", ("d1",), restore_changes(known, ("d1",), {"body": "other"}))
    total += scenario("parallel restores", [rich, d, restore, restore2],
                      {"r", "r2"}, {"body": {"restored", "other"}, "secret": {ABSENT}})
    del2 = Op("dd", "delete", ("v0",))
    total += scenario("parallel deletes", [root, d, del2], {"d1", "dd"}, {"body": {"base"}})
    model = deliver([root, d])
    expect_error(lambda: model.accept(Op("wrong", "edit", ("d1",), (Change("body", "x", ("v0",)),))))
    expect_error(lambda: model.accept(Op("wrong", "edit", ("v0",), (Change("title", "x", ("v0",)),))))
    check_transport(root, a)
    check_relay(root, a, b, b2)
    print(f"PASS: {total} permutations across 10 scenarios; protocol fault checks passed")


if __name__ == "__main__":
    main()
