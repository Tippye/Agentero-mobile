import { FileText, LoaderCircle } from "lucide-react";
import { useState } from "react";
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
import { formatLocaleTimestamp } from "@/i18n";
import { formatBytes } from "@/lib/core/background-tasks";
import { cn } from "@/lib/core/utils";
import type { ConflictResolution, PendingConflicts } from "@/lib/sync/api";

const CHOICES = [
	"keepLocal",
	"keepCloud",
	"keepBoth",
] as const satisfies readonly ConflictResolution[];

type ReviewFields = {
	pending: PendingConflicts;
	resolving: boolean;
	mobile?: boolean;
	onResolve: (resolutions: Record<string, ConflictResolution>) => void;
	onDismiss: () => void;
};

export function SyncConflictDialog({
	pending,
	open,
	...fields
}: Omit<ReviewFields, "pending"> & {
	pending: PendingConflicts | null;
	open: boolean;
}) {
	return (
		<Dialog
			open={open && pending !== null}
			onOpenChange={(next) => {
				if (!next && !fields.resolving) fields.onDismiss();
			}}
		>
			{pending ? (
				<DialogContent
					className="max-h-[calc(100dvh-2rem)] grid-rows-[auto_minmax(0,1fr)_auto] motion-reduce:animate-none sm:max-w-lg"
					showCloseButton={false}
				>
					{/* Choices belong only to the exact set of versions reviewed. */}
					<ConflictReviewFields
						key={pending.reviewId}
						pending={pending}
						{...fields}
					/>
				</DialogContent>
			) : null}
		</Dialog>
	);
}

function ConflictReviewFields({
	pending,
	resolving,
	mobile,
	onResolve,
	onDismiss,
}: ReviewFields) {
	const { t, i18n } = useTranslation("settings");
	const [choices, setChoices] = useState<Record<string, ConflictResolution>>(
		{},
	);
	const { conflicts } = pending;
	const choiceFor = (path: string) =>
		Object.hasOwn(choices, path) ? choices[path] : "keepBoth";
	const allKeepBoth = conflicts.every(
		({ path }) => choiceFor(path) === "keepBoth",
	);
	const locale = i18n.resolvedLanguage;
	const touchTarget = mobile ? "min-h-11 touch-manipulation" : undefined;

	return (
		<>
			<DialogHeader className="pr-0">
				<DialogTitle>{t("sync.conflictTitle")}</DialogTitle>
				<DialogDescription>{t("sync.conflictDescription")}</DialogDescription>
			</DialogHeader>

			<div className="agentero-scroll min-h-0 overflow-y-auto overscroll-contain">
				<ul className="flex flex-col gap-3">
					{conflicts.map((conflict) => (
						<li
							key={conflict.path}
							className="rounded-md border bg-muted/30 p-2.5"
						>
							<div className="flex items-start gap-1.5">
								<FileText
									className="mt-0.5 size-3.5 shrink-0 text-muted-foreground"
									aria-hidden
								/>
								<span className="wrap-anywhere font-medium text-xs">
									{conflict.path}
								</span>
							</div>
							<dl className="mt-2 grid grid-cols-2 gap-2 text-xs">
								<VersionDetails
									label={t("sync.conflictLocalVersion")}
									bytes={conflict.localSize}
									modifiedAt={conflict.localMtimeMs}
									locale={locale}
								/>
								<VersionDetails
									label={t("sync.conflictCloudVersion")}
									bytes={conflict.remoteSize}
									modifiedAt={conflict.remoteMtimeMs}
									locale={locale}
								/>
							</dl>
							<fieldset className="mt-2 grid min-w-0 grid-cols-3 gap-1.5">
								<legend className="sr-only">{conflict.path}</legend>
								{CHOICES.map((choice) => {
									const selected = choiceFor(conflict.path) === choice;
									return (
										<Button
											key={choice}
											type="button"
											variant={selected ? "secondary" : "outline"}
											size="sm"
											disabled={resolving}
											aria-pressed={selected}
											className={cn(
												"h-auto whitespace-normal px-1.5 py-1",
												touchTarget,
												selected && "ring-1 ring-primary/40",
											)}
											onClick={() =>
												setChoices((previous) => ({
													...previous,
													[conflict.path]: choice,
												}))
											}
										>
											{t(`sync.conflictChoice.${choice}`)}
										</Button>
									);
								})}
							</fieldset>
						</li>
					))}
				</ul>
			</div>

			<DialogFooter className="flex-row flex-wrap items-center justify-between gap-2 sm:justify-between">
				<Button
					type="button"
					variant="ghost"
					size="sm"
					className={touchTarget}
					disabled={resolving || allKeepBoth}
					onClick={() => setChoices({})}
				>
					{t("sync.conflictKeepBothAll")}
				</Button>
				<div className="ml-auto flex flex-wrap items-center justify-end gap-2">
					<Button
						type="button"
						variant="outline"
						size="sm"
						className={touchTarget}
						disabled={resolving}
						onClick={onDismiss}
					>
						{t("sync.conflictLater")}
					</Button>
					<Button
						type="button"
						size="sm"
						className={touchTarget}
						disabled={resolving}
						onClick={() =>
							onResolve(
								Object.fromEntries(
									conflicts.map(({ path }) => [path, choiceFor(path)]),
								),
							)
						}
					>
						{resolving ? (
							<LoaderCircle
								data-icon="inline-start"
								className="animate-spin motion-reduce:animate-none"
							/>
						) : null}
						{resolving ? t("sync.conflictApplying") : t("sync.conflictApply")}
					</Button>
				</div>
			</DialogFooter>
		</>
	);
}

function VersionDetails({
	label,
	bytes,
	modifiedAt,
	locale,
}: {
	label: string;
	bytes: number;
	modifiedAt: number;
	locale?: string;
}) {
	return (
		<div>
			<dt className="font-medium">{label}</dt>
			<dd className="text-muted-foreground">
				{typeof modifiedAt === "number" && modifiedAt > 0 ? (
					<time className="block" dateTime={new Date(modifiedAt).toISOString()}>
						{formatLocaleTimestamp(modifiedAt, locale)}
					</time>
				) : null}
				{formatBytes(bytes)}
			</dd>
		</div>
	);
}
