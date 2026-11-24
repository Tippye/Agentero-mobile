import { LoaderCircle, LogOut, RefreshCw } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { SiDeepseek } from "react-icons/si";
import { DshRemoteSignIn } from "@/components/remote-agent/dsh-remote-sign-in";
import { StatusBadge } from "@/components/settings/panes/agent-catalog";
import {
	SettingsGroup,
	SettingsSectionLabel,
	settingsRowClassName,
} from "@/components/settings/settings-layout";
import { Button } from "@/components/ui/button";
import type {
	RemoteAgentAccount,
	RemoteAgentCatalog,
} from "@/lib/core/bindings";
import { errorText } from "@/lib/core/error";
import { notifyError } from "@/lib/core/notify";
import { isTauri } from "@/lib/core/tauri";
import { cn } from "@/lib/core/utils";
import { credentialMessageKey } from "@/lib/remote-agent/credentials";
import { remoteAgent } from "@/lib/remote-agent/service";

/**
 * Settings → Agent → Remote agents: the dsh-remote connection module. Shares
 * the one native DSH account with the mobile Remote Agent page; sign-in is
 * third-party only (GitHub / Google / Zhihu), matching ds-harness-remote.
 */
export function DshRemoteConnectionSection() {
	const { t } = useTranslation(["settings", "common"]);
	const [catalog, setCatalog] = useState<RemoteAgentCatalog | null>(null);
	const [busy, setBusy] = useState(false);
	const catalogRead = useRef(0);

	const refresh = useCallback(async () => {
		if (!isTauri()) return;
		const read = ++catalogRead.current;
		setBusy(true);
		try {
			const value = await remoteAgent.catalog();
			if (read === catalogRead.current) setCatalog(value);
		} catch (error) {
			if (read === catalogRead.current) notifyError(errorText(error));
		} finally {
			if (read === catalogRead.current) setBusy(false);
		}
	}, []);

	const signedIn = useCallback((account: RemoteAgentAccount) => {
		// A read started before activation can still return the signed-out view.
		catalogRead.current += 1;
		setBusy(false);
		setCatalog((current) => {
			if (!current || current.account) return current;
			return {
				...current,
				account,
				persistence: account.persistence,
				credentialState: "ready",
				canClearSavedSignIn: account.persistence === "systemStore",
			};
		});
	}, []);

	useEffect(() => {
		void refresh();
	}, [refresh]);

	const signOut = useCallback(async () => {
		setBusy(true);
		try {
			await remoteAgent.logout();
			setCatalog(await remoteAgent.catalog());
		} catch (error) {
			notifyError(errorText(error));
		} finally {
			setBusy(false);
		}
	}, []);

	const account = catalog?.account ?? null;
	const credentialMessage = catalog
		? credentialMessageKey(catalog.credentialState)
		: null;

	return (
		<>
			<div className="mt-4 mb-2 flex items-center justify-between gap-2">
				<SettingsSectionLabel className="mb-0 px-0">
					{t("agent.dshRemote.section")}
				</SettingsSectionLabel>
				<Button
					type="button"
					variant="ghost"
					size="icon-xs"
					aria-label={t("agent.probe")}
					title={t("agent.probe")}
					disabled={busy || !isTauri()}
					onClick={() => void refresh()}
				>
					{busy ? (
						<LoaderCircle className="size-3.5 animate-spin" aria-hidden />
					) : (
						<RefreshCw className="size-3.5" aria-hidden />
					)}
				</Button>
			</div>
			<SettingsGroup>
				<div className={cn(settingsRowClassName, "gap-3")}>
					<div className="flex min-w-0 items-center gap-2.5">
						<SiDeepseek
							className="size-5 shrink-0 text-[#4D6BFE]"
							aria-hidden
						/>
						<span className="min-w-0 truncate font-medium text-sm">
							{t("agent.dshRemote.name")}
						</span>
						{account ? (
							<StatusBadge tone="ok" title={account.server}>
								{t("agent.dshRemote.connected")}
							</StatusBadge>
						) : catalog && !credentialMessage ? (
							<StatusBadge tone="muted">
								{t("agent.dshRemote.notConnected")}
							</StatusBadge>
						) : null}
					</div>
					{account ? (
						<div className="flex min-w-0 shrink items-center gap-2">
							<span
								className="min-w-0 truncate text-muted-foreground text-xs"
								title={
									account.loginProvider
										? `${account.account} · ${account.loginProvider}`
										: account.account
								}
							>
								{account.account}
							</span>
							<Button
								type="button"
								variant="ghost"
								size="icon-xs"
								className="size-7"
								aria-label={t("agent.dshRemote.signOut")}
								title={t("agent.dshRemote.signOut")}
								disabled={busy}
								onClick={() => void signOut()}
							>
								<LogOut className="size-3.5" aria-hidden />
							</Button>
						</div>
					) : null}
				</div>
				{!account ? (
					<DshRemoteSignIn onCatalog={setCatalog} onSignedIn={signedIn} />
				) : null}
			</SettingsGroup>
			<p className="mt-2 mb-3 px-0.5 text-muted-foreground text-xs leading-relaxed">
				{t("agent.dshRemote.hint")}
			</p>
		</>
	);
}
