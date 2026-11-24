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
				if (payload.reasoning) {
					turn.parts.push({
						kind: "reasoning",
						id: `${id}:reasoning`,
						text: payload.reasoning,
					});
				}
				if (payload.text) {
					turn.text = turn.text
						? `${turn.text}\n\n${payload.text}`
						: payload.text;
				}
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
