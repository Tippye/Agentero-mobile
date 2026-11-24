import type { AcpHistoryLine, AcpHistoryPart } from "@/lib/agent/api";
import type { RemoteEvent } from "@/lib/core/bindings";

/** One renderable piece of a remote turn, mirroring desktop transcript parts. */
export type RemoteTranscriptPart =
	| { kind: "reasoning"; id: string; text: string }
	| {
			kind: "tool";
			id: string;
			name: string;
			state: "running" | "finished" | "failed";
			input?: unknown;
			output?: string;
	  };

export type RemoteTranscriptRow =
	| { kind: "user"; id: string; text: string }
	| {
			kind: "assistant";
			id: string;
			text: string;
			parts: RemoteTranscriptPart[];
			streaming: boolean;
	  }
	| { kind: "interaction"; id: string; event: RemoteEvent };

/** Tool arguments arrive as bounded JSON text; fall back to the raw string. */
export function parseToolArguments(raw: string | null | undefined): unknown {
	if (raw == null || raw === "") return undefined;
	try {
		return JSON.parse(raw);
	} catch {
		return raw;
	}
}

const PLACEHOLDER_TOOL_NAMES = new Set(["", "Tool"]);

export function remoteToolStatus(state: string): string {
	switch (state) {
		case "finished":
			return "completed";
		case "failed":
			return "failed";
		default:
			return "in_progress";
	}
}

/** A durable chunk row and an active stream can expose the same model step. */
export class RemoteAssistantText {
	private rendered = "";
	private chunks = "";
	private stream = "";
	private attempt = "";
	private finalizedAttempt = "";
	private settled = "";

	chunk(sourceId: string, text: string): string {
		if (sourceId.startsWith("stream:")) {
			const attempt = sourceId.slice(0, sourceId.lastIndexOf(":"));
			if (attempt === this.finalizedAttempt) return "";
			if (attempt !== this.attempt) {
				this.attempt = attempt;
				this.stream = "";
			}
			this.stream += text;
			return this.append(this.stream);
		}
		this.chunks += text;
		return this.append(this.chunks);
	}

	message(text: string): string {
		const suffix = this.append(text);
		this.settled += text;
		this.finalizedAttempt = this.attempt;
		this.rendered = this.chunks = this.stream = "";
		return suffix;
	}

	get content(): string {
		return (
			this.settled +
			(this.chunks.startsWith(this.stream) ? this.chunks : this.stream)
		);
	}

	private append(candidate: string): string {
		if (!candidate.startsWith(this.rendered)) return "";
		const suffix = candidate.slice(this.rendered.length);
		this.rendered = candidate;
		return suffix;
	}
}

/** Fold durable messages and the final in-flight suffix into shared history. */
export function remoteHistoryLines(events: RemoteEvent[]): AcpHistoryLine[] {
	const lines: AcpHistoryLine[] = [];
	let current: AcpHistoryLine | undefined;
	let pending: AcpHistoryPart[] = [];
	let thought = new RemoteAssistantText();
	let answer = new RemoteAssistantText();
	const tools = new Map<string, Extract<AcpHistoryPart, { type: "tool" }>>();
	for (const { sourceStreamId, sourceId, payload } of events) {
		const id = JSON.stringify([sourceStreamId, sourceId]);
		if (payload.type === "message" && payload.role === "user") {
			current = undefined;
			lines.push({ id, kind: "user", text: payload.text });
			continue;
		}
		if (payload.type === "taskStatus") {
			if (["idle", "completed", "failed", "cancelled"].includes(payload.status))
				current = undefined;
			continue;
		}
		if (
			payload.type !== "tool" &&
			payload.type !== "assistantChunk" &&
			!(payload.type === "message" && payload.role === "assistant")
		)
			continue;
		if (payload.type === "tool") {
			const key = JSON.stringify([sourceStreamId, payload.callId]);
			const existing = tools.get(key);
			if (existing) {
				if (!PLACEHOLDER_TOOL_NAMES.has(payload.name))
					existing.tool.title = payload.name;
				if (payload.arguments != null)
					existing.tool.input = parseToolArguments(payload.arguments);
				if (payload.output != null) existing.tool.output = payload.output;
				existing.tool.status = remoteToolStatus(payload.state);
				continue;
			}
		}
		if (!current) {
			current = { id, kind: "agent", text: "", parts: [] };
			pending = [];
			thought = new RemoteAssistantText();
			answer = new RemoteAssistantText();
			lines.push(current);
		}
		if (payload.type === "tool") {
			const part: Extract<AcpHistoryPart, { type: "tool" }> = {
				type: "tool",
				tool: {
					id: payload.callId,
					title: payload.name || "Tool",
					kind: "other",
					status: remoteToolStatus(payload.state),
					input: parseToolArguments(payload.arguments),
					output: payload.output ?? undefined,
				},
			};
			tools.set(JSON.stringify([sourceStreamId, payload.callId]), part);
			current.parts?.push(part);
		} else {
			const finalize = payload.type === "message";
			const reasoning = finalize
				? (payload.reasoning ?? "")
				: (payload.reasoningDelta ?? "");
			const text = finalize ? payload.text : (payload.textDelta ?? "");
			if (finalize) {
				// A durable assistant message supersedes its archived or streamed chunks.
				current.parts = current.parts?.filter(
					(part) => !pending.includes(part),
				);
				pending = [];
				thought.message(reasoning);
				answer.message(text);
				if (reasoning)
					current.parts?.push({ type: "reasoning", text: reasoning });
				if (text) current.parts?.push({ type: "text", text });
			} else {
				appendHistoryChunk(
					current.parts,
					pending,
					"reasoning",
					thought.chunk(sourceId, reasoning),
				);
				appendHistoryChunk(
					current.parts,
					pending,
					"text",
					answer.chunk(sourceId, text),
				);
			}
			current.reasoning = thought.content || undefined;
			current.text = answer.content;
		}
	}
	return lines;
}

function appendHistoryChunk(
	parts: AcpHistoryPart[] | undefined,
	pending: AcpHistoryPart[],
	type: "text" | "reasoning",
	text: string,
): void {
	if (!text) return;
	const last = parts?.at(-1);
	if (last?.type === type && pending.at(-1) === last) {
		last.text += text;
	} else {
		const part = { type, text };
		parts?.push(part);
		pending.push(part);
	}
}

/**
 * Fold a polled snapshot into desktop-shaped transcript rows. Tool call and
 * result share `tool:{callId}` and merge into one activity row; reasoning
 * rides with its assistant message. Consecutive assistant events accumulate
 * into one turn that closes on the next human message or interaction request.
 */
export function buildRemoteTranscript(
	events: RemoteEvent[],
	sessionRunning: boolean,
): RemoteTranscriptRow[] {
	const rows: RemoteTranscriptRow[] = [];
	let current: Extract<RemoteTranscriptRow, { kind: "assistant" }> | null =
		null;
	const toolRows = new Map<
		string,
		{
			row: Extract<RemoteTranscriptPart, { kind: "tool" }>;
			turn: Extract<RemoteTranscriptRow, { kind: "assistant" }>;
		}
	>();
	const closeTurn = () => {
		if (current) current.streaming = false;
		current = null;
	};
	const openTurn = (id: string) => {
		if (!current) {
			current = { kind: "assistant", id, text: "", parts: [], streaming: true };
			rows.push(current);
		}
		return current;
	};
	for (const event of events) {
		const id = `${event.sourceStreamId}:${event.sourceId}`;
		const payload = event.payload;
		switch (payload.type) {
			case "message": {
				if (payload.role === "user") {
					closeTurn();
					rows.push({ kind: "user", id, text: payload.text });
					break;
				}
				const turn = openTurn(id);
				// A streamed turn is finalized by its durable message: replace
				// accumulated deltas instead of appending a second copy.
				const streamed = turn.id !== id && (turn.text || turn.parts.length);
				if (payload.reasoning) {
					if (streamed) {
						turn.parts = turn.parts.filter((part) => part.kind !== "reasoning");
					}
					turn.parts.push({
						kind: "reasoning",
						id: `${id}:reasoning`,
						text: payload.reasoning,
					});
				} else if (streamed) {
					turn.parts = turn.parts.filter((part) => part.kind !== "reasoning");
				}
				if (payload.text) {
					turn.text = streamed
						? payload.text
						: turn.text
							? `${turn.text}\n\n${payload.text}`
							: payload.text;
				}
				if (streamed) turn.id = id;
				break;
			}
			case "tool": {
				const turn = openTurn(id);
				const existing = toolRows.get(payload.callId);
				if (existing) {
					// The result may carry only a placeholder name; keep the
					// call's real name and arguments, take state/output from it.
					const row = existing.row;
					if (!PLACEHOLDER_TOOL_NAMES.has(payload.name)) {
						row.name = payload.name;
					}
					if (row.input === undefined) {
						row.input = parseToolArguments(payload.arguments);
					}
					row.state =
						payload.state === "failed"
							? "failed"
							: payload.state === "finished"
								? "finished"
								: "running";
					if (payload.output != null) row.output = payload.output;
					// A late result after an interaction keeps the activity in
					// its original turn instead of opening an empty new one.
					if (turn !== existing.turn && turn.parts.length === 0 && !turn.text) {
						rows.splice(rows.indexOf(turn), 1);
						current = existing.turn;
					}
				} else {
					const row: Extract<RemoteTranscriptPart, { kind: "tool" }> = {
						kind: "tool",
						id,
						name: PLACEHOLDER_TOOL_NAMES.has(payload.name)
							? "Tool"
							: payload.name,
						state:
							payload.state === "failed"
								? "failed"
								: payload.state === "finished"
									? "finished"
									: "running",
						input: parseToolArguments(payload.arguments),
						output: payload.output ?? undefined,
					};
					toolRows.set(payload.callId, { row, turn });
					turn.parts.push(row);
				}
				break;
			}
			case "assistantChunk": {
				// Live deltas accumulate into one open turn per stream; the
				// durable assistant/message later replaces them with final text.
				const turn = openTurn(`${event.sourceStreamId}:stream`);
				if (payload.reasoningDelta) {
					const last = turn.parts[turn.parts.length - 1];
					if (last?.kind === "reasoning") {
						last.text += payload.reasoningDelta;
					} else {
						turn.parts.push({
							kind: "reasoning",
							id: `${event.sourceStreamId}:stream:reasoning`,
							text: payload.reasoningDelta,
						});
					}
				}
				if (payload.textDelta) turn.text += payload.textDelta;
				break;
			}
			case "permissionRequest":
			case "questionRequest": {
				closeTurn();
				rows.push({ kind: "interaction", id, event });
				break;
			}
			default:
				// Task/connection status is surfaced by the session header.
				break;
		}
	}
	if (current) {
		(current as Extract<RemoteTranscriptRow, { kind: "assistant" }>).streaming =
			sessionRunning;
	}
	return rows;
}
