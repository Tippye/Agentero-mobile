import { describe, expect, it } from "vitest";

import type {
	EventPayload,
	RemoteEvent,
	SessionRef,
} from "@/lib/core/bindings";
import {
	buildRemoteTranscript,
	parseToolArguments,
} from "@/lib/remote-agent/transcript";

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

	it("parses bounded JSON arguments and falls back to raw text", () => {
		expect(parseToolArguments('{"a":1}')).toEqual({ a: 1 });
		expect(parseToolArguments("not json")).toBe("not json");
		expect(parseToolArguments(null)).toBeUndefined();
		expect(parseToolArguments("")).toBeUndefined();
	});
});
