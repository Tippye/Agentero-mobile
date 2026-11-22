import { ExternalLink, LoaderCircle, RefreshCw } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { EasyScholarSettingsBlock } from "@/components/settings/panes/general/easy-scholar-settings-block";
import { PaperInfoPanel } from "@/components/sidebar/paper-info-panel";
import { Button } from "@/components/ui/button";
import {
	Dialog,
	DialogContent,
	DialogDescription,
	DialogHeader,
	DialogTitle,
} from "@/components/ui/dialog";
import { MathText } from "@/components/ui/math-text";
import { TooltipProvider } from "@/components/ui/tooltip";
import { errorText } from "@/lib/core/error";
import { notifyError, notifyWarning } from "@/lib/core/notify";
import {
	openMobilePaperLink,
	paperJumpUrl,
	readMobilePaper,
} from "@/lib/mobile/paper-import";
import { setPaperTags, syncPublication } from "@/lib/paper/api";
import type { PaperTag } from "@/lib/paper/tags";
import type { PaperMetadata } from "@/lib/paper/types";
import {
	loadSettings,
	saveSettingsAsync,
	subscribeSettings,
} from "@/lib/settings";

export type MobilePaperInfoProps = {
	open: boolean;
	onOpenChange: (open: boolean) => void;
	paper: PaperMetadata | null;
	vaultPath: string | null;
	onUpdated?: (paper: PaperMetadata) => void | Promise<void>;
};

export function MobilePaperInfo({
	open,
	onOpenChange,
	paper,
	vaultPath,
	onUpdated,
}: MobilePaperInfoProps) {
	const { t } = useTranslation("sidebar");
	return (
		<Dialog open={open} onOpenChange={onOpenChange}>
			<DialogContent
				className="flex flex-col overflow-hidden sm:max-w-xl"
				style={{
					maxHeight:
						"calc(100dvh - env(safe-area-inset-top) - env(safe-area-inset-bottom) - 2rem)",
					top: "calc(50% + (env(safe-area-inset-top) - env(safe-area-inset-bottom)) / 2)",
				}}
			>
				<DialogHeader>
					<DialogTitle>{t("paperInfo.info")}</DialogTitle>
					<DialogDescription className="sr-only">
						{paper?.title || t("paperInfo.selectPrompt")}
					</DialogDescription>
				</DialogHeader>
				{open && paper && vaultPath ? (
					<PaperInfoBody
						key={`${vaultPath}\0${paper.path}`}
						paper={paper}
						vaultPath={vaultPath}
						onUpdated={onUpdated}
					/>
				) : null}
			</DialogContent>
		</Dialog>
	);
}

function PaperInfoBody({
	paper,
	vaultPath,
	onUpdated,
}: {
	paper: PaperMetadata;
	vaultPath: string;
	onUpdated?: MobilePaperInfoProps["onUpdated"];
}) {
	const { t } = useTranslation(["sidebar", "settings", "mobile"]);
	const [meta, setMeta] = useState(paper);
	const [settings, setSettings] = useState(loadSettings);
	const [keyDraft, setKeyDraft] = useState(settings.easyScholarKey);
	const [busy, setBusy] = useState(false);
	const active = useRef(false);
	const busyRef = useRef(false);
	const updated = useRef(onUpdated);
	updated.current = onUpdated;
	useEffect(() => {
		active.current = true;
		return () => {
			active.current = false;
		};
	}, []);
	useEffect(() => setMeta(paper), [paper]);
	useEffect(() => subscribeSettings(setSettings), []);
	useEffect(
		() => setKeyDraft(settings.easyScholarKey),
		[settings.easyScholarKey],
	);

	const publish = async (next: PaperMetadata) => {
		if (!active.current || next.path !== paper.path) return;
		setMeta(next);
		await updated.current?.(next);
	};
	const mutate = async (operation: () => Promise<void>) => {
		if (busyRef.current) return;
		busyRef.current = true;
		setBusy(true);
		try {
			await operation();
		} catch (error) {
			if (active.current) notifyError(errorText(error));
		} finally {
			busyRef.current = false;
			if (active.current) setBusy(false);
		}
	};
	const saveTags = (tags: PaperTag[]) =>
		mutate(async () => {
			await publish(await setPaperTags(vaultPath, paper.path, tags));
		});
	const refreshPublication = () =>
		mutate(async () => {
			const report = await syncPublication(vaultPath, [paper.path]);
			const fresh = await readMobilePaper(vaultPath, paper.path);
			await publish(fresh);
			if (active.current && report.metricsFailed)
				notifyWarning(
					t("papersLibrary.syncPublicationPartial", {
						failed: report.metricsFailed,
					}),
				);
		});
	const commitKey = (value: string) =>
		mutate(async () => {
			const saved = await saveSettingsAsync({
				...loadSettings(),
				easyScholarKey: value.trim(),
			});
			if (active.current) {
				setSettings(saved);
				setKeyDraft(saved.easyScholarKey);
			}
		});
	const jump = (url: string) => {
		void openMobilePaperLink(url).catch((error) => {
			if (active.current) notifyError(errorText(error));
		});
	};
	const fields = [
		["mobile:paperInfo.year", meta.year],
		["paperInfo.editMeta.fieldDoi", meta.doi],
		["mobile:paperInfo.isbn", meta.isbn],
		["mobile:paperInfo.issn", meta.issn],
		["mobile:paperInfo.pmid", meta.pmid],
		["paperInfo.editMeta.fieldVolume", meta.volume],
		["paperInfo.editMeta.fieldIssue", meta.issue],
		["paperInfo.editMeta.fieldPages", meta.pages],
		["paperInfo.editMeta.fieldPublisher", meta.publisher],
		["mobile:paperInfo.place", meta.place],
		["mobile:paperInfo.series", meta.series],
		["mobile:paperInfo.language", meta.language],
		["mobile:paperInfo.bibtexKey", meta.bibtex_key],
		["mobile:paperInfo.citations", meta.citation_count],
	] as const;
	const doi = meta.doi
		?.trim()
		.replace(/^(?:https?:\/\/(?:dx\.)?doi\.org\/|doi:\s*)/i, "");
	const links = [
		{
			label: t("paperInfo.editMeta.fieldDoi"),
			url: doi
				? `https://doi.org/${encodeURIComponent(doi).replace(/%2F/gi, "/")}`
				: null,
		},
		{
			label: t("paperInfo.editMeta.fieldHtmlUrl"),
			url: paperJumpUrl(meta.html_url),
		},
		{ label: t("mobile:paperInfo.source"), url: paperJumpUrl(meta.source_url) },
	].filter((link): link is { label: string; url: string } => link.url !== null);

	return (
		<TooltipProvider>
			<div className="min-h-0 flex-1 space-y-4 overflow-y-auto overscroll-contain pb-1">
				<PaperInfoPanel
					meta={meta}
					mobile
					onTagsChange={busy ? undefined : saveTags}
				/>
				<dl className="grid grid-cols-[minmax(0,auto)_minmax(0,1fr)] gap-x-4 gap-y-2 px-3 text-sm">
					{fields
						.filter(([, value]) => value != null && String(value).trim())
						.map(([label, value]) => (
							<div key={label} className="contents">
								<dt className="text-muted-foreground">{t(label)}</dt>
								<dd className="break-words select-text">{value}</dd>
							</div>
						))}
				</dl>
				{meta.abstract ? (
					<section className="px-3">
						<h3 className="mb-2 text-sm font-medium">
							{t("paperInfo.editMeta.fieldAbstract")}
						</h3>
						<MathText
							text={meta.abstract}
							className="whitespace-pre-wrap break-words text-sm leading-relaxed"
						/>
					</section>
				) : null}
				{links.length ? (
					<div className="flex flex-wrap gap-2 px-3">
						{links.map(({ label, url }) => (
							<Button
								key={label}
								type="button"
								variant="outline"
								className="min-h-11"
								onClick={() => jump(url)}
							>
								<ExternalLink className="size-4" aria-hidden />
								{label}
							</Button>
						))}
					</div>
				) : null}
				<section className="border-t pt-4">
					<Button
						type="button"
						variant="outline"
						className="mb-4 min-h-11 w-full"
						disabled={busy}
						onClick={() => void refreshPublication()}
					>
						{busy ? (
							<LoaderCircle className="size-4 animate-spin" aria-hidden />
						) : (
							<RefreshCw className="size-4" aria-hidden />
						)}
						{t("papersLibrary.syncPublication")}
					</Button>
					<fieldset disabled={busy} className="min-w-0">
						<legend className="sr-only">
							{t("settings:general.easyScholar.section")}
						</legend>
						<EasyScholarSettingsBlock
							savedKey={settings.easyScholarKey}
							keyDraft={keyDraft}
							onKeyDraftChange={setKeyDraft}
							onCommitKey={commitKey}
							ranks={settings.easyScholarRanks}
							onRanksChange={(next) =>
								mutate(async () => {
									const saved = await saveSettingsAsync({
										...loadSettings(),
										easyScholarRanks: next,
									});
									if (active.current) setSettings(saved);
								})
							}
							onOpenKeyPage={() =>
								jump("https://www.easyscholar.cc/console/user/open")
							}
						/>
					</fieldset>
				</section>
			</div>
		</TooltipProvider>
	);
}
