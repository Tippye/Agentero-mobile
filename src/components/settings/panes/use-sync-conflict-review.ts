import { useLayoutEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { errorMessage, notifyError, notifySuccess } from "@/lib/core/notify";
import {
	type ConflictResolution,
	isSyncConflictPending,
	type PendingConflicts,
	syncResolveConflicts,
} from "@/lib/sync/api";

export function useSyncConflictReview({
	vaultPath,
	pending,
	refresh,
	onSynced,
	onError,
}: {
	vaultPath: string | null;
	pending: PendingConflicts | null;
	refresh: () => Promise<void>;
	onSynced?: () => void | Promise<void>;
	onError?: (error: unknown) => void;
}) {
	const { t } = useTranslation("settings");
	const [dismissedReviewId, setDismissedReviewId] = useState<string | null>(
		null,
	);
	const [resolving, setResolving] = useState(false);
	const active = useRef<{ resolving: boolean } | null>(null);
	useLayoutEffect(() => {
		void vaultPath;
		active.current = { resolving: false };
		setDismissedReviewId(null);
		setResolving(false);
		return () => {
			active.current = null;
		};
	}, [vaultPath]);

	const reviewId = pending?.reviewId;
	useLayoutEffect(() => {
		if (!reviewId) setDismissedReviewId(null);
	}, [reviewId]);

	const reportError = (error: unknown) => {
		if (onError) onError(error);
		else notifyError(errorMessage(error), { id: `sync:${vaultPath}` });
	};

	const apply = async (resolutions: Record<string, ConflictResolution>) => {
		const owner = active.current;
		if (!vaultPath || !reviewId || !owner || owner.resolving) return;
		owner.resolving = true;
		setResolving(true);
		try {
			const outcome = await syncResolveConflicts(
				vaultPath,
				reviewId,
				resolutions,
			);
			if (active.current !== owner) return;
			setDismissedReviewId(reviewId);
			notifySuccess(
				t("sync.done", { up: outcome.uploaded, down: outcome.downloaded }),
			);
			if (outcome.conflictCopies.length > 0) {
				notifySuccess(
					t("sync.conflicts", { count: outcome.conflictCopies.length }),
				);
			}
			await onSynced?.();
		} catch (error) {
			if (active.current === owner && !isSyncConflictPending(error))
				reportError(error);
		} finally {
			if (active.current === owner) {
				try {
					// A stale review is replaced by the latest durable set, if any.
					await refresh();
				} catch (error) {
					if (active.current === owner) reportError(error);
				} finally {
					if (active.current === owner) {
						owner.resolving = false;
						setResolving(false);
					}
				}
			}
		}
	};

	return {
		open: Boolean(reviewId && reviewId !== dismissedReviewId),
		resolving,
		reopen: () => setDismissedReviewId(null),
		dismiss: () => {
			if (!active.current?.resolving && reviewId)
				setDismissedReviewId(reviewId);
		},
		apply,
	};
}
