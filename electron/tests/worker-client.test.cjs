const assert = require("node:assert/strict");
const path = require("node:path");
const { test } = require("node:test");
const { EventEmitter, once } = require("node:events");
const { WorkerClient } = require("../out/tests/main/worker-client.js");

function setup(t, overrides = {}) {
  const events = [];
  const emitter = new EventEmitter();
  const client = new WorkerClient({
    command: process.execPath,
    argsPrefix: [path.join(__dirname, "fixtures/worker.cjs")],
    cwd: __dirname,
    onEvent: (event) => { events.push(event); emitter.emit(`event:${event.kind}`, event); },
    stopTimeoutMs: 100,
    ...overrides
  });
  t.after(() => client.shutdown());
  return { client, events, wait: (kind) => once(emitter, `event:${kind}`) };
}

test("reuses the worker and isolates transfer IDs, including before readiness", { timeout: 5000 }, async (t) => {
  const { client, events, wait } = setup(t);
  const firstEnd = wait("process_end");
  client.start({ command: "send", files: ["first"] });
  assert.throws(() => client.start({ command: "send", files: [] }), /already running/);
  await firstEnd;
  assert.equal(client.busy, false);
  const secondEnd = wait("process_end");
  client.start({ command: "receive", output: ".", target: "ticket" });
  await secondEnd;
  const status = events.filter((event) => event.kind === "status");
  assert.equal(status.length, 2);
  assert.equal(status[0].value, status[1].value, "both transfers should use the same process");
  assert.notEqual(status[0].transfer_id, status[1].transfer_id);
  assert.equal(events.filter((event) => event.kind === "process_end").length, 2);
  assert.ok(events.every((event) => event.transfer_id !== "previous-transfer"));
});

test("cancel keeps the healthy process available", { timeout: 5000 }, async (t) => {
  const { client, events, wait } = setup(t);
  const firstStatus = wait("status");
  client.start({ command: "send", files: [], to: "wait" });
  await firstStatus;
  const canceled = wait("process_end");
  assert.equal(client.cancel(), true);
  assert.equal(client.busy, true, "must wait for cleanup acknowledgement");
  assert.equal((await canceled)[0].value, "canceled");
  const end = wait("process_end");
  client.start({ command: "send", files: [] });
  await end;
  const status = events.filter((event) => event.kind === "status");
  assert.equal(status[0].value, status[1].value);
});

test("crash reports failure once and restarts for the next transfer", { timeout: 5000 }, async (t) => {
  const { client, events, wait } = setup(t);
  const failed = wait("process_end");
  client.start({ command: "send", files: [], to: "crash" });
  assert.equal((await failed)[0].value, "failed");
  assert.equal(events.filter((event) => event.kind === "error").length, 1);
  const completed = wait("process_end");
  client.start({ command: "send", files: [] });
  assert.equal((await completed)[0].value, "completed");
});

test("unresponsive cancellation kills the process and allows retry", { timeout: 5000 }, async (t) => {
  const { client, events, wait } = setup(t);
  const firstStatus = wait("status");
  client.start({ command: "send", files: [], to: "ignore-cancel" });
  await firstStatus;
  const canceled = wait("process_end");
  client.cancel();
  assert.equal((await canceled)[0].value, "canceled");
  const completed = wait("process_end");
  client.start({ command: "send", files: [] });
  await completed;
  const status = events.filter((event) => event.kind === "status");
  assert.notEqual(status[0].value, status[1].value);
});

test("missing executable releases an in-flight transfer", { timeout: 5000 }, async (t) => {
  const { client, events, wait } = setup(t, { command: path.join(__dirname, "does-not-exist") });
  const failed = wait("process_end");
  client.start({ command: "send", files: [] });
  assert.equal((await failed)[0].value, "failed");
  assert.equal(client.busy, false);
  assert.equal(events.filter((event) => event.kind === "error").length, 1);
});

test("forwards nearby records outside transfers and answers the active offer", { timeout: 5000 }, async (t) => {
  const nearby = [];
  const found = new EventEmitter();
  const { client, wait } = setup(t, {
    onNearby: (event) => { nearby.push(event); found.emit("found"); }
  });
  const seen = once(found, "found");
  client.setBrowsing(true);
  await seen;
  assert.deepEqual(nearby, [{ kind: "nearby_found", peer_id: "peer", message: "Desk", lan: true }]);
  assert.equal(client.answer(true), false, "no transfer, no offer to answer");
  const started = wait("status");
  client.start({ command: "receive", output: ".", nearby: true, to: "wait" });
  await started;
  const answered = wait("status");
  assert.equal(client.answer(true), true);
  assert.equal((await answered)[0].value, "answer:true");
});
