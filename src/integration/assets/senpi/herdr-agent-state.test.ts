import { describe, expect, test } from "bun:test";
import { mkdtemp, rm } from "node:fs/promises";
import net from "node:net";
import { tmpdir } from "node:os";
import path from "node:path";
import extension, { createReporter, sendSignal } from "./herdr-agent-state";

const count = (activeCount: number) => ({ source: "senpi-codemode", activeCount });

function fixture() {
  let active = false;
  const reports: { active: boolean; seq: number }[] = [];
  const reporter = createReporter(async (next, seq) => {
    active = next;
    reports.push({ active: next, seq });
  });
  return { reporter, reports, isActive: () => active };
}

describe("Senpi detached eval authority", () => {
  test("reports live detached work and releases when the last cell settles", async () => {
    const f = fixture();
    await f.reporter.start(true);
    await f.reporter.observe(count(1));
    expect(f.isActive()).toBe(true);
    await f.reporter.observe(count(0));
    expect(f.isActive()).toBe(false);
    expect(f.reports.map((r) => r.active)).toEqual([true, false]);
    expect(f.reports[1]?.seq).toBeGreaterThan(f.reports[0]?.seq ?? 0);
  });

  test("full snapshots do not double count or republish unchanged authority", async () => {
    const f = fixture();
    await f.reporter.start(true);
    for (const n of [1, 2, 2, 1]) await f.reporter.observe(count(n));
    expect(f.isActive()).toBe(true);
    expect(f.reports).toHaveLength(1);
    await f.reporter.observe(count(0));
    expect(f.isActive()).toBe(false);
  });

  test("preserves a producer snapshot delivered before session-start binding", async () => {
    const f = fixture();
    await f.reporter.observe(count(1));
    expect(f.isActive()).toBe(false);
    await f.reporter.start(true);
    expect(f.isActive()).toBe(true);
  });

  test("headless contexts and unrelated or malformed sources cannot claim the pane", async () => {
    const f = fixture();
    await f.reporter.start(false);
    await f.reporter.observe(count(1));
    expect(f.reports).toHaveLength(0);
    await f.reporter.stop();
    await f.reporter.start(true);
    for (const event of [
      { source: "terminal", activeCount: 1 }, count(-1), count(0.5),
      count(Number.NaN), count(Number.POSITIVE_INFINITY), null, {},
    ]) await f.reporter.observe(event);
    expect(f.isActive()).toBe(false);
    expect(f.reports).toHaveLength(0);
  });

  test("session teardown invalidates reports that have not started", async () => {
    const f = fixture();
    await f.reporter.start(true);
    const pending = f.reporter.observe(count(1));
    const stopped = f.reporter.stop();
    await Promise.all([pending, stopped]);
    expect(f.isActive()).toBe(false);
    expect(f.reports).toHaveLength(0);
  });

  test("teardown releases an in-flight old generation before new work reports", async () => {
    const entered = Promise.withResolvers<void>();
    const acknowledge = Promise.withResolvers<void>();
    const states: boolean[] = [];
    let first = true;
    const reporter = createReporter(async (active) => {
      states.push(active);
      if (active && first) {
        first = false;
        entered.resolve();
        await acknowledge.promise;
      }
    });
    await reporter.start(true);
    const oldReport = reporter.observe(count(1));
    await entered.promise;
    const stop = reporter.stop();
    const snapshot = reporter.observe(count(1));
    const start = reporter.start(true);
    acknowledge.resolve();
    await Promise.all([oldReport, stop, snapshot, start]);
    expect(states).toEqual([true, false, true]);
    await reporter.stop();
    expect(states.at(-1)).toBe(false);
  });

  test("failed acknowledgement still requires release of possibly applied authority", async () => {
    let active = false;
    let first = true;
    const reporter = createReporter(async (next) => {
      active = next;
      if (first) {
        first = false;
        throw new Error("acknowledgement lost after applying report");
      }
    });
    await reporter.start(true);
    await expect(reporter.observe(count(1))).rejects.toThrow("acknowledgement lost");
    expect(active).toBe(true);
    await reporter.observe(count(0));
    expect(active).toBe(false);
  });
});

async function withServer(
  reply: (request: { id: string; method: string; params: Record<string, unknown> }) => unknown,
  run: (socket: string) => Promise<void>,
) {
  const dir = await mkdtemp(path.join(tmpdir(), "herdr-senpi-"));
  const socketPath = process.platform === "win32" ? path.basename(dir) : path.join(dir, "s.sock");
  const endpoint = process.platform === "win32" ? `\\\\.\\pipe\\${socketPath}` : socketPath;
  const server = net.createServer((socket) => {
    let data = "";
    socket.on("data", (chunk) => {
      data += chunk.toString();
      if (!data.includes("\n")) return;
      socket.end(JSON.stringify(reply(JSON.parse(data.trim()))) + "\n");
    });
  });
  try {
    await new Promise<void>((resolve, reject) => {
      server.once("error", reject);
      server.listen(endpoint, resolve);
    });
    await run(socketPath);
  } finally {
    await new Promise<void>((resolve) => server.close(() => resolve()));
    await rm(dir, { recursive: true, force: true });
  }
}

test("rejects mismatched API acknowledgement rather than assuming delivery", async () => {
  await withServer(
    () => ({ id: "another-request", result: { type: "ok" } }),
    (socket) => expect(sendSignal(socket, "w1:p1", true, 11)).rejects.toThrow(),
  );
});

test("report and release use acknowledged API methods without an idle report", async () => {
  const methods: string[] = [];
  let active = false;
  await withServer((request) => {
    expect(request.params.pane_id).toBe("w1:p1");
    expect(request.params.agent).toBe("omo");
    expect(request.params.source).toBe("herdr:senpi-detached-eval");
    methods.push(request.method);
    if (request.method === "pane.report_agent") {
      expect(request.params.state).toBe("working");
      active = true;
    } else {
      expect(request.params.state).toBeUndefined();
      active = false;
    }
    return { id: request.id, result: { type: "ok" } };
  }, async (socket) => {
    await sendSignal(socket, "w1:p1", true, 12);
    expect(active).toBe(true);
    await sendSignal(socket, "w1:p1", false, 13);
    expect(active).toBe(false);
  });
  expect(methods).toEqual(["pane.report_agent", "pane.release_agent"]);
});

test("rejects explicit API errors", async () => {
  await withServer(
    (request) => ({ id: request.id, error: { code: "rejected", message: "not owner" } }),
    (socket) => expect(sendSignal(socket, "w1:p1", true, 14)).rejects.toThrow(),
  );
});

function extensionHost() {
  type Api = Parameters<typeof extension>[0];
  type Handler = Parameters<Api["on"]>[1];
  const handlers = new Map<string, Handler>();
  const events = new Map<string, (event: unknown) => Promise<void>>();
  const api: Api = {
    events: {
      on(channel, handler) {
        events.set(channel, handler);
        return () => { events.delete(channel); };
      },
    },
    on(event, handler) { handlers.set(event, handler); },
  };
  return {
    api,
    events,
    async lifecycle(name: string, hasUI = true) {
      await handlers.get(name)?.({}, { hasUI });
    },
    async snapshot(activeCount: number) {
      await events.get("wake_source_state")?.(count(activeCount));
    },
  };
}

test("real extension wiring retains detached work after foreground end and releases on switch", async () => {
  let active = false;
  await withServer((request) => {
    active = request.method === "pane.report_agent";
    return { id: request.id, result: { type: "ok" } };
  }, async (socket) => {
    const host = extensionHost();
    extension(host.api, { HERDR_ENV: "1", HERDR_SOCKET_PATH: socket, HERDR_PANE_ID: "w1:p1" });
    await host.snapshot(1);
    expect(active).toBe(false);
    await host.lifecycle("session_start");
    expect(active).toBe(true);
    await host.lifecycle("agent_end");
    expect(active).toBe(true);
    await host.lifecycle("session_before_switch");
    expect(active).toBe(false);
    await host.snapshot(1);
    expect(active).toBe(false);
    await host.lifecycle("session_start");
    expect(active).toBe(true);
    await host.lifecycle("session_shutdown");
    expect(active).toBe(false);
    expect(host.events.size).toBe(0);
  });
});

test("team children cannot attach an inherited parent-pane reporter", async () => {
  const host = extensionHost();
  extension(host.api, {
    HERDR_ENV: "1", HERDR_SOCKET_PATH: "unused", HERDR_PANE_ID: "w1:p1",
    SENPI_TASK_MEMBER: "reviewer",
  });
  await host.lifecycle("session_start");
  await host.snapshot(1);
  expect(host.events.size).toBe(0);
});
