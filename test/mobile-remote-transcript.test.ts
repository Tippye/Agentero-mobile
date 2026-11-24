import { emitTo } from "@tauri-apps/api/event";
import { describe, expect, it, vi } from "vitest";

import type {
	EventPayload,
	RemoteEvent,
	SessionRef,
} from "@/lib/core/bindings";
import { RemoteAgentTransport } from "@/lib/remote-agent/agent-transport";
import { remoteAgent } from "@/lib/remote-agent/service";
import {
	buildRemoteTranscript,
	parseToolArguments,
	RemoteAssistantText,
	remoteHistoryLines,
} from "@/lib/remote-agent/transcript";

vi.mock("@tauri-apps/api/event", () => ({ emitTo: vi.fn() }));
vi.mock("@tauri-apps/api/webviewWindow", () => ({
	getCurrentWebviewWindow: () => ({ label: "main" }),
}));
vi.mock("@/lib/remote-agent/service", () => ({
	remoteAgent: {
		workspaces: vi.fn(),
		sessions: vi.fn(),
		read: vi.fn(),
		localOperations: vi.fn(),
		execute: vi.fn(),
	},
}));

const session: SessionRef = {
	workspace: {
		host: {
			connection: { providerId: "ds-harness-remote", connectionId: "launch" },
			hostId: "host",
		},
		backendId: "harness",
		workspaceId: "workspace",
	},
	sessionId: "session",
};

let seq = 0;
function event(payload: EventPayload, sourceId?: string): RemoteEvent {
	seq += 1;
	return {
		session,
		sourceStreamId: "stream",
		sourceId: sourceId ?? `seq:${seq}`,
		cursor: null,
		payload,
	};
}

describe("remote transcript folding", () => {
	it("merges tool call and result into one row keyed by callId", () => {
		const rows = buildRemoteTranscript(
			[
				event({ type: "message", role: "user", text: "hi" }),
				event(
					{
						type: "tool",
						callId: "call-1",
						name: "read_file",
						state: "running",
						arguments: '{"path":"a.md"}',
					},
					"tool:call-1",
				),
				event(
					{
						type: "tool",
						callId: "call-1",
						name: "Tool",
						state: "finished",
						output: "contents",
					},
					"tool:call-1",
				),
				event({ type: "message", role: "assistant", text: "done" }),
			],
			false,
		);
		expect(rows.map((row) => row.kind)).toEqual(["user", "assistant"]);
		const turn = rows[1];
		if (turn.kind !== "assistant") throw new Error("expected assistant");
		expect(turn.text).toBe("done");
		expect(turn.parts).toHaveLength(1);
		const part = turn.parts[0];
		if (part.kind !== "tool") throw new Error("expected tool");
		expect(part.name).toBe("read_file");
		expect(part.state).toBe("finished");
		expect(part.input).toEqual({ path: "a.md" });
		expect(part.output).toBe("contents");
		expect(turn.streaming).toBe(false);
	});

	it("keeps reasoning with its assistant message and marks failed tools", () => {
		const rows = buildRemoteTranscript(
			[
				event({
					type: "message",
					role: "assistant",
					text: "answer",
					reasoning: "thinking hard",
				}),
				event(
					{
						type: "tool",
						callId: "call-2",
						name: "bash",
						state: "failed",
						output: "boom",
					},
					"tool:call-2",
				),
			],
			false,
		);
		expect(rows).toHaveLength(1);
		const turn = rows[0];
		if (turn.kind !== "assistant") throw new Error("expected assistant");
		expect(turn.parts.map((part) => part.kind)).toEqual(["reasoning", "tool"]);
		const tool = turn.parts[1];
		if (tool.kind !== "tool") throw new Error("expected tool");
		expect(tool.state).toBe("failed");
		expect(tool.output).toBe("boom");
	});

	it("closes turns on human messages and keeps interactions inline", () => {
		const permission = event({
			type: "permissionRequest",
			requestId: "req",
			description: "run?",
			options: ["allowed-once", "rejected"],
		});
		const rows = buildRemoteTranscript(
			[
				event({ type: "message", role: "assistant", text: "first" }),
				permission,
				event({ type: "message", role: "user", text: "next" }),
				event({ type: "message", role: "assistant", text: "second" }),
			],
			true,
		);
		expect(rows.map((row) => row.kind)).toEqual([
			"assistant",
			"interaction",
			"user",
			"assistant",
		]);
		const last = rows[3];
		if (last.kind !== "assistant") throw new Error("expected assistant");
		expect(last.streaming).toBe(true);
		const first = rows[0];
		if (first.kind !== "assistant") throw new Error("expected assistant");
		expect(first.streaming).toBe(false);
	});

	it("keeps a late tool result in its original turn after an interaction", () => {
		const rows = buildRemoteTranscript(
			[
				event(
					{
						type: "tool",
						callId: "call-3",
						name: "bash",
						state: "running",
					},
					"tool:call-3",
				),
				event({
					type: "questionRequest",
					requestId: "q",
					questions: [],
				}),
				event(
					{
						type: "tool",
						callId: "call-3",
						name: "Tool",
						state: "finished",
						output: "ok",
					},
					"tool:call-3",
				),
			],
			true,
		);
		expect(rows.map((row) => row.kind)).toEqual(["assistant", "interaction"]);
		const turn = rows[0];
		if (turn.kind !== "assistant") throw new Error("expected assistant");
		expect(turn.parts).toHaveLength(1);
		const part = turn.parts[0];
		if (part.kind !== "tool") throw new Error("expected tool");
		expect(part.state).toBe("finished");
		expect(part.output).toBe("ok");
	});

	it("accumulates live deltas into one streaming turn and finalizes without duplication", () => {
		const rows = buildRemoteTranscript(
			[
				event({ type: "message", role: "user", text: "hi" }),
				event({ type: "assistantChunk", reasoningDelta: "think " }, "chunk:5"),
				event({ type: "assistantChunk", reasoningDelta: "hard" }, "chunk:6"),
				event({ type: "assistantChunk", textDelta: "Hel" }, "chunk:7"),
				event({ type: "assistantChunk", textDelta: "lo" }, "chunk:8"),
			],
			true,
		);
		expect(rows.map((row) => row.kind)).toEqual(["user", "assistant"]);
		const live = rows[1];
		if (live.kind !== "assistant") throw new Error("expected assistant");
		expect(live.text).toBe("Hello");
		expect(live.streaming).toBe(true);
		expect(live.parts).toHaveLength(1);
		const reasoning = live.parts[0];
		if (reasoning.kind !== "reasoning") throw new Error("expected reasoning");
		expect(reasoning.text).toBe("think hard");

		const finalized = buildRemoteTranscript(
			[
				event({ type: "message", role: "user", text: "hi" }),
				event(
					{ type: "assistantChunk", reasoningDelta: "think hard" },
					"chunk:5",
				),
				event({ type: "assistantChunk", textDelta: "Hello" }, "chunk:7"),
				event({
					type: "message",
					role: "assistant",
					text: "Hello",
					reasoning: "think hard",
				}),
			],
			false,
		);
		expect(finalized).toHaveLength(2);
		const done = finalized[1];
		if (done.kind !== "assistant") throw new Error("expected assistant");
		expect(done.text).toBe("Hello");
		expect(done.streaming).toBe(false);
		expect(done.parts).toHaveLength(1);
		const finalReasoning = done.parts[0];
		if (finalReasoning.kind !== "reasoning")
			throw new Error("expected reasoning");
		expect(finalReasoning.text).toBe("think hard");
	});

	it("folds active deltas and archived chunks into the durable assistant message", () => {
		const answer = new RemoteAssistantText();
		const thought = new RemoteAssistantText();
		expect(answer.chunk("stream:one:0", "Hel")).toBe("Hel");
		expect(thought.chunk("stream:one:1", "Think")).toBe("Think");
		expect(answer.chunk("chunk:2", "Hello")).toBe("lo");
		expect(thought.chunk("chunk:3", "Thinking")).toBe("ing");
		expect(answer.message("Hello!")).toBe("!");
		expect(thought.message("Thinking")).toBe("");
		expect(answer.chunk("stream:one:4", "stale attempt")).toBe("");
		expect(answer.content).toBe("Hello!");
		const lines = remoteHistoryLines([
			event({ type: "assistantChunk", reasoningDelta: "Thinking" }, "chunk:3"),
			event({ type: "assistantChunk", textDelta: "Hello" }, "chunk:4"),
			event({
				type: "message",
				role: "assistant",
				text: "Hello!",
				reasoning: "Thinking",
			}),
			event(
				{ type: "assistantChunk", textDelta: " A second step" },
				"stream:two:0",
			),
		]);
		expect(lines).toHaveLength(1);
		expect(lines[0]).toMatchObject({
			text: "Hello! A second step",
			reasoning: "Thinking",
			parts: [
				{ type: "reasoning", text: "Thinking" },
				{ type: "text", text: "Hello!" },
				{ type: "text", text: " A second step" },
			],
		});
	});

	it("emits scoped completion for a turn cancelled before assistant output", async () => {
		const host = {
			host: session.workspace.host,
			home: "/home/test",
			name: "Host",
			online: true,
		};
		const transport = new RemoteAgentTransport(host, {
			connection: host.host.connection,
			capabilities: {
				operations: ["listSessions", "sendMessage", "readSession"],
				authentication: [],
				backends: ["harness"],
				attachments: null,
			},
		});
		vi.mocked(remoteAgent.workspaces).mockResolvedValue([
			{
				workspace: session.workspace,
				name: "Papers",
				path: "/home/test/Agentero/Papers",
			},
		]);
		vi.mocked(remoteAgent.sessions).mockResolvedValue([
			{ session, title: "Papers", status: "cancelled" },
		]);
		const oldCancel = event({ type: "taskStatus", status: "cancelled" });
		const newCancel = event({ type: "taskStatus", status: "cancelled" });
		const baseline = {
			info: { session, title: "Papers", status: "cancelled" as const },
			events: [oldCancel],
			cursor: null,
		};
		vi.mocked(remoteAgent.read)
			.mockResolvedValueOnce(baseline)
			.mockResolvedValueOnce({ ...baseline, events: [oldCancel, newCancel] });
		vi.mocked(remoteAgent.localOperations).mockResolvedValue([]);
		vi.mocked(remoteAgent.execute).mockResolvedValue({ status: "accepted" });
		const { sessions } = await transport.listSessions();
		const accepted = await transport.run(
			{ sessionId: sessions[0].sessionId, prompt: "Try again" },
			"runtime",
		);
		expect(emitTo).not.toHaveBeenCalled();
		accepted.follow();
		await vi.waitFor(() => {
			expect(emitTo).toHaveBeenCalledExactlyOnceWith(
				{ kind: "WebviewWindow", label: "main" },
				"agent:completed",
				expect.objectContaining({
					sessionId: "runtime",
					providerSessionId: sessions[0].sessionId,
					stopReason: "cancelled",
					content: "",
					reasoning: "",
				}),
			);
		});
	});

	it("parses bounded JSON arguments and falls back to raw text", () => {
		expect(parseToolArguments('{"a":1}')).toEqual({ a: 1 });
		expect(parseToolArguments("not json")).toBe("not json");
		expect(parseToolArguments(null)).toBeUndefined();
		expect(parseToolArguments("")).toBeUndefined();
	});
});
