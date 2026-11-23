import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { events } from "@/lib/core/bindings";
import { errorText } from "@/lib/core/error";
import { isLikelyNetworkFailure, notifyError } from "@/lib/core/notify";
import { toSafeDisposer } from "@/lib/core/tauri-events";
import {
	fetchMobilePaperIfDeferred,
	mobileSyncPaperId,
	setMobilePaperPinned,
} from "@/lib/mobile/paper-sync";
import type { PaperMetadata } from "@/lib/paper/types";
import {
	isSyncConflictPending,
	isSyncProgressPhase,
	type PaperAvailability,
	type PendingConflicts,
	type SyncProgressEvent,
	syncGetStatus,
	syncListPaperAvailability,
} from "@/lib/sync/api";

export type MobilePaperSyncState = {
	pinned: boolean;
	offline: boolean;
	busy: boolean;
};

/** Inventory is local and advisory; the Host rechecks credentials and hashes on actions. */
export function useMobilePaperSync({
	vaultPath,
	papers,
	active,
	onChanged,
}: {
	vaultPath: string | null;
	papers: PaperMetadata[];
	active: boolean;
	onChanged: () => Promise<void>;
}) {
	const { t } = useTranslation("settings");
	const [inventory, setInventory] = useState<PaperAvailability[]>([]);
	const [enabled, setEnabled] = useState(false);
	const [busy, setBusy] = useState(new Set<string>());
	const busyRef = useRef(new Set<string>());
	const generation = useRef(0);
	const requestId = useRef(0);
	const [progress, setProgress] = useState<SyncProgressEvent | null>(null);
	const [pending, setPending] = useState<{
		vaultPath: string;
		conflicts: PendingConflicts;
	} | null>(null);
	const refresh = useCallback(async () => {
		if (!vaultPath) return;
		const request = generation.current;
		const ticket = ++requestId.current;
		const status = await syncGetStatus(vaultPath);
		const supported =
			status.configured &&
			(status.config?.backend === "s3" || status.config?.backend === "webdav");
		if (request !== generation.current || ticket !== requestId.current) return;
		const conflicts = supported ? status.pendingConflicts : null;
		setPending(conflicts?.conflicts.length ? { vaultPath, conflicts } : null);
		setEnabled(
			Boolean(
				supported &&
					!["locked", "unavailable", "missing", "invalid"].includes(
						status.credentialState ?? "",
					),
			),
		);
		const next = supported ? await syncListPaperAvailability(vaultPath) : [];
		if (request === generation.current && ticket === requestId.current)
			setInventory(next);
	}, [vaultPath]);
	useEffect(() => {
		void vaultPath; // Clear inventory and invalidate old requests when the Vault changes.
		generation.current += 1;
		setInventory([]);
		setEnabled(false);
		setProgress(null);
		setPending(null);
		return () => {
			generation.current += 1;
		};
	}, [vaultPath]);
	useEffect(() => {
		if (active) void refresh().catch((error) => notifyError(errorText(error)));
	}, [active, refresh]);
	useEffect(() => {
		if (!vaultPath) return;
		let listening = true;
		const report = (error: unknown) => {
			if (listening) notifyError(errorText(error), { id: `sync:${vaultPath}` });
		};
		const visible = () => {
			if (document.visibilityState === "visible") void refresh().catch(report);
		};
		window.addEventListener("focus", visible);
		document.addEventListener("visibilitychange", visible);
		const stateListener = events.syncState.listen(({ payload }) => {
			if (
				!listening ||
				payload.vaultPath !== vaultPath ||
				payload.status === "syncing"
			)
				return;
			setProgress(null);
			if (payload.status === "error") {
				const detail = payload.error ?? t("sync.statusError");
				const message =
					payload.errorCode === "sync_no_conditional_writes"
						? t("sync.noConditionalWrites")
						: payload.errorCode === "sync_unreachable" ||
								isLikelyNetworkFailure(detail)
							? t("sync.unreachable")
							: detail;
				notifyError(message, {
					id: `sync:${vaultPath}`,
					...(message === detail ? {} : { description: detail }),
				});
			}
			void refresh().catch(report);
		});
		// Read after subscribing so a pause during startup cannot slip between the read and the listener.
		void stateListener
			.then(() => {
				if (listening) return refresh();
			})
			.catch(report);
		const offs = [
			toSafeDisposer(stateListener),
			toSafeDisposer(
				events.syncProgress.listen(({ payload }) => {
					if (
						payload.vaultPath === vaultPath &&
						isSyncProgressPhase(payload.phase)
					)
						setProgress({ ...payload, phase: payload.phase });
				}),
			),
		];
		return () => {
			listening = false;
			window.removeEventListener("focus", visible);
			document.removeEventListener("visibilitychange", visible);
			for (const off of offs) off();
		};
	}, [vaultPath, refresh, t]);
	const pendingConflicts =
		pending?.vaultPath === vaultPath ? pending.conflicts : null;
	const setPinned = useCallback(
		async (paper: PaperMetadata, pinned: boolean) => {
			if (!vaultPath || busyRef.current.has(paper.id)) return;
			if (pinned && pendingConflicts) {
				notifyError(t("sync.statusConflict"), { id: `sync:${vaultPath}` });
				return;
			}
			busyRef.current.add(paper.id);
			setBusy(new Set(busyRef.current));
			try {
				await setMobilePaperPinned(vaultPath, paper, pinned);
			} catch (error) {
				if (!isSyncConflictPending(error)) notifyError(errorText(error));
			} finally {
				busyRef.current.delete(paper.id);
				setBusy(new Set(busyRef.current));
				await refresh().catch((error) => notifyError(errorText(error)));
				await onChanged();
			}
		},
		[vaultPath, pendingConflicts, refresh, onChanged, t],
	);
	const fetchMissing = useCallback(
		async (paper: PaperMetadata) => {
			if (!vaultPath || pendingConflicts) return false;
			try {
				const fetched = await fetchMobilePaperIfDeferred(vaultPath, paper);
				if (fetched) {
					await onChanged();
					await refresh();
				}
				return fetched;
			} catch (error) {
				if (!isSyncConflictPending(error)) throw error;
				await refresh();
				return false;
			}
		},
		[vaultPath, pendingConflicts, onChanged, refresh],
	);
	const states = useMemo(() => {
		const result: Record<string, MobilePaperSyncState> = {};
		if (!vaultPath || !enabled) return result;
		const byId = new Map(inventory.map((entry) => [entry.paperId, entry]));
		for (const paper of papers) {
			try {
				const entry = byId.get(mobileSyncPaperId(vaultPath, paper));
				if (!entry) continue;
				const pdfs = entry.assets.filter((asset) => asset.kind === "pdf");
				result[paper.id] = {
					pinned: entry.pinned,
					offline: pdfs.length > 0 && pdfs.every((asset) => asset.materialized),
					busy: busy.has(paper.id),
				};
			} catch {
				/* Invalid catalog paths never receive native file actions. */
			}
		}
		return result;
	}, [vaultPath, enabled, inventory, papers, busy]);
	return {
		states,
		setPinned,
		fetchMissing,
		pendingConflicts,
		refresh,
		progressText: progress
			? t(`sync.phase.${progress.phase}`, {
					current: progress.current,
					total: progress.total,
				})
			: null,
	};
}
