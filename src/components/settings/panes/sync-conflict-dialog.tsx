import { FileText } from "lucide-react";
import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import {
	Dialog,
	DialogContent,
	DialogDescription,
	DialogFooter,
	DialogHeader,
	DialogTitle,
} from "@/components/ui/dialog";
import type { ConflictResolution, PendingConflicts } from "@/lib/sync/api";

/** Which side of a divergent file the reader keeps. */
const CHOICES = [
	"keepLocal",
	"keepCloud",
	"keepBoth",
] as const satisfies readonly ConflictResolution[];

/**
 * Steam-cloud-save style conflict prompt.
 *
 * A pass that stops with `sync_conflict_pending` leaves the conflicting paths
 * durable in `pendingConflicts`; nothing was applied or published, so every
 * file is still exactly as the user last left it on each device. The dialog
 * asks per file and defaults to keeping both, because that is the only choice
 * that cannot destroy work.
 */
export function SyncConflictDialog({
	pending,
	resolving,
	onResolve,
	onDismiss,
}: {
	pending: PendingConflicts | null;
	/** A resolved pass is running: choices are locked and the footer spinner shows. */
	resolving: boolean;
	onResolve: (
		resolutions: Record<string, ConflictResolution>,
		defaultResolution: ConflictResolution,
	) => void;
	/** Reader closed the dialog: the pause stays, and sync re-asks next pass. */
	onDismiss: () => void;
}) {
	const { t } = useTranslation("settings");
	const [choices, setChoices] = useState<Record<string, ConflictResolution>>(
		{},
	);

	// `pending` is replaced wholesale on every paused pass, so the per-file map
	// is keyed by path and rebuilt lazily instead of in an effect.
	const resolved = useMemo(() => {
		const out: Record<string, ConflictResolution> = {};
		for (const conflict of pending?.conflicts ?? []) {
			out[conflict.path] = choices[conflict.path] ?? "keepBoth";
		}
		return out;
	}, [pending, choices]);

	const conflicts = pending?.conflicts ?? [];
	const allKeepBoth = conflicts.every(
		(conflict) => resolved[conflict.path] === "keepBoth",
	);

	return (
		<Dialog
			open={pending !== null}
			onOpenChange={(open) => {
				if (!open) onDismiss();
			}}
		>
			<DialogContent className="max-w-lg">
				<DialogHeader>
					<DialogTitle>{t("sync.conflictTitle")}</DialogTitle>
					<DialogDescription>
						{t("sync.conflictDescription", { count: conflicts.length })}
					</DialogDescription>
				</DialogHeader>

				<div className="agentero-scroll max-h-[45vh] overflow-y-auto overscroll-contain">
					<ul className="flex flex-col gap-3">
						{conflicts.map((conflict) => (
							<li
								key={conflict.path}
								className="rounded-md border bg-muted/30 p-2.5"
							>
								<div className="flex items-center gap-1.5">
									<FileText
										className="size-3.5 shrink-0 text-muted-foreground"
										aria-hidden
									/>
									<span className="break-all font-medium text-xs">
										{conflict.path}
									</span>
								</div>
								<div className="mt-2 grid grid-cols-3 gap-1.5">
									{CHOICES.map((choice) => (
										<ChoiceButton
											key={choice}
											label={t(`sync.conflictChoice.${choice}`)}
											selected={resolved[conflict.path] === choice}
											disabled={resolving}
											onClick={() =>
												setChoices((prev) => ({
													...prev,
													[conflict.path]: choice,
												}))
											}
										/>
									))}
								</div>
							</li>
						))}
					</ul>
				</div>

				<DialogFooter className="flex-row items-center justify-between gap-2 sm:justify-between">
					<Button
						type="button"
						variant="ghost"
						size="sm"
						disabled={resolving || allKeepBoth}
						onClick={() => {
							const next: Record<string, ConflictResolution> = {};
							for (const conflict of conflicts) {
								next[conflict.path] = "keepBoth";
							}
							setChoices(next);
						}}
					>
						{t("sync.conflictKeepBothAll")}
					</Button>
					<div className="flex items-center gap-2">
						<Button
							type="button"
							variant="outline"
							size="sm"
							disabled={resolving}
							onClick={onDismiss}
						>
							{t("sync.conflictLater")}
						</Button>
						<Button
							type="button"
							size="sm"
							disabled={resolving}
							onClick={() => onResolve(resolved, "keepBoth")}
						>
							{resolving ? t("sync.conflictApplying") : t("sync.conflictApply")}
						</Button>
					</div>
				</DialogFooter>
			</DialogContent>
		</Dialog>
	);
}

function ChoiceButton({
	label,
	selected,
	disabled,
	onClick,
}: {
	label: string;
	selected: boolean;
	disabled: boolean;
	onClick: () => void;
}) {
	return (
		<Button
			type="button"
			variant={selected ? "secondary" : "outline"}
			size="sm"
			disabled={disabled}
			aria-pressed={selected}
			className={selected ? "ring-1 ring-primary/40" : undefined}
			onClick={onClick}
		>
			{label}
		</Button>
	);
}
