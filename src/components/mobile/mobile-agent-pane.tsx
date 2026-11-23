import { useTranslation } from "react-i18next";
import { AgentCommonRows } from "@/components/settings/agent-common-rows";
import { AgentPersonalPromptBlock } from "@/components/settings/panes/agent/agent-personal-prompt-block";
import { DshRemoteConnectionSection } from "@/components/settings/panes/agent/dsh-remote-connection";
import {
	PageTitle,
	SettingsGroup,
} from "@/components/settings/settings-layout";
import type { AppSettings } from "@/lib/settings";

/**
 * Mobile Agent settings: app-level agent preferences plus the dsh-remote
 * connection module. Local CLI agents, Node.js tooling and model seats are
 * desktop-only surfaces and stay out of the mobile shell.
 */
export function MobileAgentPane({
	settings,
	patch,
}: {
	settings: AppSettings;
	patch: (p: Partial<AppSettings>) => void;
}) {
	const { t } = useTranslation("settings");
	return (
		<>
			<PageTitle title={t("nav.agent")} />
			<SettingsGroup>
				<AgentCommonRows settings={settings} patch={patch} idSuffix="-m" />
			</SettingsGroup>
			<DshRemoteConnectionSection />
			<AgentPersonalPromptBlock settings={settings} patch={patch} />
		</>
	);
}
