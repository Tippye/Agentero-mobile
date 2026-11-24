import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
	appendAssistantLine,
	appendStreamChunk,
	appendUserLine,
	completeStream,
} from "@/components/mobile/chat-lines";
import type {
	AcpLoadSessionResult,
	AgentFailedEvent,
	AgentLine,
	AgentPermissionRequest,
	AgentStatusEvent,
	AgentStreamEvent,
	AgentTurnPhase,
} from "@/components/mobile/types";
import type { AgentResultPayload } from "@/lib/agent/api";
import { bridgeRpc, listenBridgeEvent } from "@/lib/bridge/client";
import { toSafeDisposer } from "@/lib/core/tauri-events";

/**
 * Chat state machine for the mobile agent page: subscribes to the bridged
 * agent events of the current session, restores the timeline on launch and
 * when returning to the foreground, and submits new prompts.
 */
export function useMobileAgentChat({
	agentId,
	sessionId,
	onSessionId,
}: {
	agentId: string | null;
	sessionId: string | null;
	onSessionId: (sessionId: string | null) => void;
}) {
	const { t } = useTranslation("mobile");
	const [lines, setLines] = useState<AgentLine[]>([]);
	const [sending, setSending] = useState(false);
	const [restoring, setRestoring] = useState(false);
	const [permission, setPermission] = useState<AgentPermissionRequest | null>(
		null,
	);
	/** Loading phase of the in-flight turn (label under the sending state). */
	const [phase, setPhase] = useState<AgentStatusEvent | null>(null);
	const sessionRef = useRef<string | null>(sessionId);
	const runRef = useRef<string | null>(null);
	const submittingRef = useRef(false);
	const restoreGeneration = useRef(0);
	const pendingPermissionRef = useRef<AgentPermissionRequest | null>(null);
	pendingPermissionRef.current = permission;

	const restore = useCallback(
		async (target: string) => {
			if (submittingRef.current) return;
			const generation = ++restoreGeneration.current;
			setRestoring(true);
			try {
				const history = await bridgeRpc<AcpLoadSessionResult>(
					"agent_load_session",
					{
						agentId: agentId ?? undefined,
						sessionId: target,
					},
				);
				if (generation !== restoreGeneration.current) return;
				sessionRef.current = history.sessionId;
				onSessionId(history.sessionId);
				setLines(
					history.lines.map((line) => ({
						id: line.id,
						role: line.kind === "user" ? "user" : "assistant",
						text: line.text,
					})),
				);
			} catch {
				// Session history is best-effort; keep the current timeline.
			} finally {
				if (generation === restoreGeneration.current) setRestoring(false);
			}
		},
		[agentId, onSessionId],
	);

	useEffect(() => {
		if (sessionRef.current) void restore(sessionRef.current);
		return () => {
			restoreGeneration.current += 1;
		};
	}, [restore]);

	useEffect(() => {
		const onVisible = () => {
			if (document.visibilityState !== "visible") return;
			if (sessionRef.current) void restore(sessionRef.current);
		};
		document.addEventListener("visibilitychange", onVisible);
		return () => document.removeEventListener("visibilitychange", onVisible);
	}, [restore]);

	useEffect(() => {
		let active = true;
		const unlisten = [
			toSafeDisposer(
				listenBridgeEvent<AgentStreamEvent>("agent:stream", (event) => {
					if (!active || event.sessionId !== runRef.current) return;
					setPhase(null);
					setLines((current) => appendStreamChunk(current, event.chunk));
				}),
			),
			toSafeDisposer(
				listenBridgeEvent<AgentStatusEvent>("agent:status", (event) => {
					if (!active || event.sessionId !== runRef.current) return;
					if (
						event.phase !== "starting" &&
						event.phase !== "waiting-model" &&
						event.phase !== "reconnecting"
					) {
						return;
					}
					setPhase({ ...event, phase: event.phase as AgentTurnPhase });
				}),
			),
			toSafeDisposer(
				listenBridgeEvent<AgentResultPayload>("agent:completed", (event) => {
					if (!active || event.sessionId !== runRef.current) return;
					runRef.current = null;
					submittingRef.current = false;
					sessionRef.current = event.providerSessionId ?? null;
					onSessionId(sessionRef.current);
					setSending(false);
					setPhase(null);
					setLines((current) => completeStream(current, event.content));
				}),
			),
			toSafeDisposer(
				listenBridgeEvent<AgentFailedEvent>("agent:failed", (event) => {
					if (!active || event.sessionId !== runRef.current) return;
					runRef.current = null;
					submittingRef.current = false;
					setSending(false);
					setPhase(null);
					setLines((current) =>
						appendAssistantLine(current, event.error || t("agent.failed")),
					);
				}),
			),
			toSafeDisposer(
				listenBridgeEvent<AgentPermissionRequest>(
					"agent:permission-request",
					(event) => {
						if (!active || event.sessionId !== runRef.current) return;
						setPermission(event);
					},
				),
			),
		];
		return () => {
			active = false;
			for (const off of unlisten) off();
			const pending = pendingPermissionRef.current;
			if (pending) {
				void bridgeRpc("agent_respond_permission", {
					requestId: pending.requestId,
					optionId: null,
				});
			}
		};
	}, [onSessionId, t]);

	const clearSession = useCallback(() => {
		const current = runRef.current;
		restoreGeneration.current += 1;
		runRef.current = null;
		sessionRef.current = null;
		submittingRef.current = false;
		setRestoring(false);
		setLines([]);
		setSending(false);
		setPhase(null);
		setPermission(null);
		onSessionId(null);
		if (current) {
			void bridgeRpc("agent_cancel_run", { sessionId: current }).catch(
				() => undefined,
			);
		}
	}, [onSessionId]);

	const respondToPermission = useCallback((optionId: string | null) => {
		const pending = pendingPermissionRef.current;
		if (!pending) return;
		setPermission(null);
		void bridgeRpc("agent_respond_permission", {
			requestId: pending.requestId,
			optionId,
		});
	}, []);

	const send = useCallback(
		async (
			value: string,
			{
				modelId,
				workflow,
				target,
			}: { modelId?: string; workflow?: string; target?: string } = {},
		) => {
			const next = value.trim();
			if (!next || submittingRef.current || restoring) return;
			submittingRef.current = true;
			const generation = ++restoreGeneration.current;
			setLines((previous) => appendUserLine(previous, next));
			setSending(true);
			setPhase(null);
			try {
				const accepted = await bridgeRpc<{ sessionId: string }>(
					"agent_run_once",
					{
						agentId: agentId ?? undefined,
						sessionId: sessionRef.current ?? undefined,
						prompt: next,
						workflow,
						target,
						modelId,
						forceModel: modelId !== undefined,
						permissionMode: "ask",
					},
				);
				if (generation !== restoreGeneration.current) {
					void bridgeRpc("agent_cancel_run", {
						sessionId: accepted.sessionId,
					}).catch(() => undefined);
					return;
				}
				runRef.current = accepted.sessionId;
			} catch (error) {
				if (generation !== restoreGeneration.current) return;
				submittingRef.current = false;
				setSending(false);
				setPhase(null);
				setLines((current) =>
					appendAssistantLine(
						current,
						error instanceof Error ? error.message : t("agent.failed"),
					),
				);
			}
		},
		[agentId, restoring, t],
	);

	return {
		lines,
		sending,
		restoring,
		permission,
		phase,
		restore,
		send,
		clearSession,
		respondToPermission,
	};
}
