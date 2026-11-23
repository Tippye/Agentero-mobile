import { getVersion } from "@tauri-apps/api/app";
import {
	ArrowLeft,
	BatteryCharging,
	Bot,
	CloudUpload,
	Info,
	Languages,
	LayoutTemplate,
	LoaderCircle,
	Paintbrush,
	Star,
} from "lucide-react";
import { lazy, Suspense, useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import agenteroAppIcon from "@/assets/agentero-app-icon.svg";
import { MobileAgentPane } from "@/components/mobile/mobile-agent-pane";
import { MobileBackgroundSettingsPanel } from "@/components/mobile/mobile-background-settings";
import { MobileDownloadSettings } from "@/components/mobile/mobile-download-settings";
import { AppearancePane } from "@/components/settings/panes/appearance-pane";
import { LayoutPane } from "@/components/settings/panes/layout-pane";
import { TranslatePane } from "@/components/settings/panes/translate-pane";
import { PageTitle } from "@/components/settings/settings-layout";
import { Button } from "@/components/ui/button";
import { errorMessage, notifyError } from "@/lib/core/notify";
import { openExternalUrl } from "@/lib/core/open-external";
import type { MobileSettingsSection } from "@/lib/mobile/settings";
import {
	type AppSettings,
	loadSettings,
	saveSettingsAsync,
	subscribeSettings,
} from "@/lib/settings";
import { isRemoteVaultHandle } from "@/lib/vault/remote/remote-vault";

// The sync pane pulls in the full storage configuration surface; load it on
// demand like the desktop shell so opening settings stays cheap.
const SyncPane = lazy(() =>
	import("@/components/settings/panes/sync-pane").then((module) => ({
		default: module.SyncPane,
	})),
);

const GITHUB_REPO_URL = "https://github.com/poco-ai/Agentero";

type MobileSettingsLabel =
	| "settings:nav.appearance"
	| "settings:nav.agent"
	| "settings:nav.translate"
	| "settings:nav.layout"
	| "settings:nav.sync"
	| "mobile:background.title"
	| "settings:nav.about";

const NAV: {
	id: MobileSettingsSection;
	icon: typeof Paintbrush;
	label: MobileSettingsLabel;
}[] = [
	{ id: "appearance", icon: Paintbrush, label: "settings:nav.appearance" },
	{ id: "agent", icon: Bot, label: "settings:nav.agent" },
	{ id: "translate", icon: Languages, label: "settings:nav.translate" },
	{ id: "layout", icon: LayoutTemplate, label: "settings:nav.layout" },
	{ id: "sync", icon: CloudUpload, label: "settings:nav.sync" },
	{ id: "background", icon: BatteryCharging, label: "mobile:background.title" },
	{ id: "about", icon: Info, label: "settings:nav.about" },
];

export type MobileSettingsPageProps = {
	open: boolean;
	section: MobileSettingsSection;
	onSectionChange: (section: MobileSettingsSection) => void;
	onClose: () => void;
	vaultPath: string | null;
	onSynced?: () => void | Promise<void>;
	onReviewSyncConflicts?: () => void;
};

/** Full-screen in-shell settings surface — aggregates every mobile option. */
export function MobileSettingsPage({
	open,
	section,
	onSectionChange,
	onClose,
	vaultPath,
	onSynced,
	onReviewSyncConflicts,
}: MobileSettingsPageProps) {
	const { t } = useTranslation(["settings", "mobile", "common"]);
	const [settings, setSettings] = useState(loadSettings);
	const [version, setVersion] = useState<string | null>(null);
	useEffect(() => subscribeSettings(setSettings), []);
	useEffect(() => {
		if (!open) return;
		let active = true;
		void getVersion()
			.then((value) => {
				if (active) setVersion(value);
			})
			.catch(() => {});
		return () => {
			active = false;
		};
	}, [open]);
	const patch = useCallback((partial: Partial<AppSettings>) => {
		const next = { ...loadSettings(), ...partial };
		setSettings(next);
		void saveSettingsAsync(next).catch((error) =>
			notifyError(errorMessage(error)),
		);
	}, []);
	if (!open) return null;
	return (
		<section
			className="mobile-safe-area fixed inset-0 z-40 flex h-dvh flex-col bg-background text-foreground"
			aria-label={t("settings:title")}
		>
			<header className="flex min-h-16 shrink-0 items-center gap-2 border-b px-4">
				<Button
					type="button"
					variant="ghost"
					size="icon"
					className="size-11 shrink-0"
					aria-label={t("common:close")}
					onClick={onClose}
				>
					<ArrowLeft className="size-5" />
				</Button>
				<h1 className="min-w-0 flex-1 truncate font-semibold text-base">
					{t("settings:title")}
				</h1>
			</header>
			<div className="flex min-h-0 flex-1 flex-col sm:flex-row">
				<nav className="flex shrink-0 gap-1 overflow-x-auto border-b p-2 sm:w-56 sm:flex-col sm:overflow-y-auto sm:border-r sm:border-b-0 sm:py-2">
					{NAV.map(({ id, icon: Icon, label }) => {
						const active = section === id;
						return (
							<Button
								key={id}
								type="button"
								variant={active ? "secondary" : "ghost"}
								aria-current={active ? "page" : undefined}
								className="min-h-11 shrink-0 justify-start gap-2 px-3 sm:w-full"
								onClick={() => onSectionChange(id)}
							>
								<Icon className="size-4 shrink-0" aria-hidden />
								<span className="whitespace-nowrap">{t(label)}</span>
							</Button>
						);
					})}
				</nav>
				<div className="agentero-scroll min-h-0 flex-1 overflow-y-auto overscroll-contain [&_[data-slot=select-trigger]]:max-w-[45vw] [&_[data-slot=select-trigger]]:min-h-11 [&_[data-slot=select-trigger]]:min-w-0 [&_[data-slot=select-trigger]]:w-40 [&_button]:touch-manipulation [&_input]:min-h-11">
					<div className="mx-auto w-full max-w-3xl px-4 py-4 sm:px-6">
						{section === "appearance" ? (
							<AppearancePane
								theme={settings.theme}
								uiTheme={settings.uiTheme}
								locale={settings.locale}
								uiScale={settings.uiScale}
								soundEnabled={settings.soundEnabled}
								editorFontSize={settings.editorFontSize}
								interfaceFontFamily={settings.interfaceFontFamily}
								textFontFamily={settings.textFontFamily}
								monoFontFamily={settings.monoFontFamily}
								editorLineHeight={settings.editorLineHeight}
								showEditorToolbar={settings.showEditorToolbar}
								wikiEmbedFontSize={settings.wikiEmbedFontSize}
								markdownMaxWidth={settings.markdownMaxWidth}
								plazaEnabled={settings.plazaEnabled}
								pdfScrollStrategy={settings.pdfScrollStrategy}
								pdfVimKeys={settings.pdfVimKeys}
								pdfSpreadMode={settings.pdfSpreadMode}
								pdfSentenceHover={settings.pdfSentenceHover}
								pdfCommentDisplay={settings.pdfCommentDisplay}
								pdfTranslateTab={settings.pdfTranslateTab}
								vimMode={settings.vimMode}
								patch={patch}
							/>
						) : section === "agent" ? (
							<MobileAgentPane settings={settings} patch={patch} />
						) : section === "translate" ? (
							<TranslatePane
								settings={settings}
								patch={patch}
								vaultPath={vaultPath}
								apiOnly
							/>
						) : section === "layout" ? (
							<LayoutPane
								settings={settings}
								patch={patch}
								hostContext={{ kind: "local" }}
								apiOnly
							/>
						) : section === "sync" ? (
							<>
								<Suspense
									fallback={
										<div
											role="status"
											className="flex min-h-40 items-center justify-center gap-2 text-muted-foreground text-sm"
										>
											<LoaderCircle className="size-5 animate-spin" />
											{t("mobile:sync.loading")}
										</div>
									}
								>
									<SyncPane
										vaultPath={vaultPath}
										mobile
										onSynced={onSynced}
										onReviewConflicts={onReviewSyncConflicts}
									/>
								</Suspense>
								{vaultPath && !isRemoteVaultHandle(vaultPath) ? (
									<MobileDownloadSettings vaultPath={vaultPath} />
								) : null}
							</>
						) : section === "background" ? (
							<MobileBackgroundSettingsPanel active />
						) : (
							<>
								<PageTitle title={t("settings:about.title")} />
								<div className="flex items-center gap-3">
									<img
										src={agenteroAppIcon}
										alt=""
										className="size-12 shrink-0"
										aria-hidden
									/>
									<div className="min-w-0">
										<p className="font-semibold text-base">Agentero</p>
										{version ? (
											<p className="text-muted-foreground text-sm">{`v${version}`}</p>
										) : null}
									</div>
								</div>
								<Button
									type="button"
									variant="outline"
									size="sm"
									className="mt-4 min-h-11"
									onClick={() => openExternalUrl(GITHUB_REPO_URL)}
								>
									<Star data-icon="inline-start" className="size-4" />
									{t("settings:about.starGithub")}
								</Button>
							</>
						)}
					</div>
				</div>
			</div>
		</section>
	);
}
