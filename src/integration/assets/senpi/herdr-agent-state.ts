// installed by herdr
// managed by herdr; reinstall this integration after updating herdr.
// HERDR_INTEGRATION_ID=senpi
// HERDR_INTEGRATION_VERSION=1

import net from "node:net";

const source = "herdr:senpi-detached-eval";
let sequence = Date.now() * 1000;

export type Publish = (active: boolean, seq: number) => Promise<void>;

class IntegrationError extends Error {
  constructor(message: string, options?: ErrorOptions) {
    super(message, options);
    this.name = "HerdrSenpiIntegrationError";
  }
}

function acknowledged(value: unknown, id: string): boolean {
  return value !== null && typeof value === "object"
    && "id" in value && value.id === id
    && !("error" in value)
    && "result" in value && value.result !== null && typeof value.result === "object"
    && "type" in value.result && value.result.type === "ok";
}

export function sendSignal(
  socketPath: string, paneId: string, active: boolean, seq: number,
): Promise<void> {
  const id = `${source}:${seq}`;
  const request = {
    id,
    method: active ? "pane.report_agent" : "pane.release_agent",
    params: {
      pane_id: paneId, source, agent: "omo", seq,
      ...(active ? { state: "working" } : {}),
    },
  };
  const endpoint = process.platform === "win32" ? `\\\\.\\pipe\\${socketPath}` : socketPath;
  return new Promise<void>((resolve, reject) => {
    const socket = net.createConnection(endpoint);
    let response = "";
    let settled = false;
    const finish = (error?: Error) => {
      if (settled) return;
      settled = true;
      socket.destroy();
      if (error) reject(error);
      else resolve();
    };
    socket.setTimeout(2000, () => finish(new IntegrationError("Herdr acknowledgement timed out")));
    socket.once("error", (error) => finish(error));
    socket.once("end", () => finish(new IntegrationError("Herdr closed before acknowledgement")));
    socket.once("connect", () => socket.write(`${JSON.stringify(request)}\n`));
    socket.on("data", (chunk: Buffer) => {
      response += chunk.toString("utf8");
      if (response.length > 65536) {
        finish(new IntegrationError("Herdr response exceeded 64 KiB"));
        return;
      }
      const end = response.indexOf("\n");
      if (end < 0) return;
      try {
        const value: unknown = JSON.parse(response.slice(0, end));
        finish(acknowledged(value, id) ? undefined : new IntegrationError("Herdr rejected the state report"));
      } catch (error) {
        if (!(error instanceof SyntaxError)) throw error;
        finish(new IntegrationError("Invalid Herdr acknowledgement", { cause: error }));
      }
    });
  });
}

function detachedCount(event: unknown): number | undefined {
  if (event === null || typeof event !== "object"
    || !("source" in event) || event.source !== "senpi-codemode"
    || !("activeCount" in event) || typeof event.activeCount !== "number"
    || !Number.isSafeInteger(event.activeCount) || event.activeCount < 0) return undefined;
  return event.activeCount;
}

export function createReporter(publish: Publish) {
  let enabled = false;
  let count: number | undefined;
  let generation = 0;
  let published: "released" | "working" | "unknown" = "released";
  let queue = Promise.resolve();

  const enqueue = (operation: () => Promise<void>) => {
    // A later release must still run after an earlier request failed.
    // Each returned rejection is observed by Senpi's extension error boundary.
    queue = queue.then(operation, operation);
    return queue;
  };
  const publishState = async (active: boolean) => {
    if (published === (active ? "working" : "released")) return;
    // A request can be applied even if its acknowledgement is lost.
    // Keep release mandatory until its own acknowledgement succeeds.
    published = "unknown";
    await publish(active, ++sequence);
    published = active ? "working" : "released";
  };
  const reconcile = () => {
    const current = generation;
    return enqueue(async () => {
      if (current !== generation) return;
      await publishState(enabled && count !== undefined && count > 0);
    });
  };

  return {
    start(hasUI: boolean): Promise<void> {
      enabled = hasUI;
      // Do not erase a producer snapshot delivered earlier in session_start.
      return reconcile();
    },
    observe(event: unknown): Promise<void> {
      const next = detachedCount(event);
      if (next === undefined) return Promise.resolve();
      count = next;
      return reconcile();
    },
    stop(): Promise<void> {
      enabled = false;
      count = undefined;
      generation += 1;
      // Unconditionally reconcile the OLD ownership before a new session reports.
      return enqueue(() => publishState(false));
    },
  };
}

type SessionContext = { readonly hasUI: boolean };
type SessionHandler = (event: unknown, context: SessionContext) => Promise<void>;
type LifecycleEvent = "session_start" | "session_before_switch" | "session_before_fork" | "session_shutdown";
interface SenpiExtensionApi {
  readonly events: {
    on(channel: string, handler: (event: unknown) => Promise<void>): () => void;
  };
  on(event: LifecycleEvent, handler: SessionHandler): void;
}

type Environment = {
  readonly HERDR_ENV?: string;
  readonly HERDR_SOCKET_PATH?: string;
  readonly HERDR_PANE_ID?: string;
  readonly SENPI_TASK_MEMBER?: string;
};

// Senpi's extension loader requires a default entry point.
export default function senpiDetachedEval(
  pi: SenpiExtensionApi, env: Environment = process.env,
): void {
  const socketPath = env.HERDR_SOCKET_PATH;
  const paneId = env.HERDR_PANE_ID;
  if (env.HERDR_ENV !== "1" || !socketPath || !paneId || env.SENPI_TASK_MEMBER) return;
  const reporter = createReporter((active, seq) => sendSignal(socketPath, paneId, active, seq));
  const unsubscribe = pi.events.on("wake_source_state", (event) => reporter.observe(event));
  pi.on("session_start", (_event, ctx) => reporter.start(ctx.hasUI));
  pi.on("session_before_switch", () => reporter.stop());
  pi.on("session_before_fork", () => reporter.stop());
  pi.on("session_shutdown", async () => {
    try {
      await reporter.stop();
    } finally {
      unsubscribe();
    }
  });
}
