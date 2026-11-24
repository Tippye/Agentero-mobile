import {
	BookOpen,
	ChevronDown,
	Columns2,
	FileText,
	History,
	MessagesSquare,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import type { MobileReaderMode } from "@/components/mobile/types";
import { Button } from "@/components/ui/button";
import {
	DropdownMenu,
	DropdownMenuContent,
	DropdownMenuLabel,
	DropdownMenuRadioGroup,
	DropdownMenuRadioItem,
	DropdownMenuSeparator,
	DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import type { AgentDescriptor } from "@/lib/agent/api";
import { cn } from "@/lib/core/utils";
import type {
	MobileWorkspacePreset,
	MobileWorkspaceWidth,
} from "@/lib/mobile/workspace";

const MODES: Array<{ id: MobileReaderMode; icon: typeof BookOpen }> = [
	{ id: "pdf", icon: BookOpen },
	{ id: "notes", icon: FileText },
];

export function MobileReaderModeToggle({
	mode,
	onChange,
	preset = "read",
	widthClass = "narrow",
	onPresetChange,
}: {
	mode: MobileReaderMode;
	onChange: (mode: MobileReaderMode) => void;
	preset?: MobileWorkspacePreset;
	widthClass?: MobileWorkspaceWidth;
	onPresetChange?: (preset: MobileWorkspacePreset) => void;
}) {
	const { t } = useTranslation("mobile");
	if (onPresetChange) {
		const choices: {
			id: MobileWorkspacePreset;
			icon: typeof BookOpen;
			label: string;
		}[] = [
			{
				id: "read",
				icon: BookOpen,
				label: widthClass === "narrow" ? t("reader.pdf") : t("workspace.read"),
			},
			{
				id: "compare",
				icon: Columns2,
				label:
					widthClass === "narrow" ? t("reader.notes") : t("workspace.compare"),
			},
			{
				id: "discuss",
				icon: MessagesSquare,
				label:
					widthClass === "narrow" ? t("tabs.agent") : t("workspace.discuss"),
			},
		];
		return (
			<div
				role="toolbar"
				aria-label={t("workspace.presets")}
				className="flex shrink-0 gap-0.5 rounded-lg border bg-muted p-0.5"
			>
				{choices.map(({ id, icon: Icon, label }) => (
					<button
						key={id}
						type="button"
						aria-label={label}
						aria-pressed={preset === id}
						onClick={() => onPresetChange(id)}
						className={cn(
							"flex min-h-11 min-w-11 items-center justify-center gap-2 rounded-md px-2",
							preset === id && "bg-background shadow-raised",
						)}
					>
						<Icon className="size-4" />
						{widthClass !== "narrow" ? (
							<span className="text-sm">{label}</span>
						) : null}
					</button>
				))}
			</div>
		);
	}
	return (
		<div className="flex shrink-0 rounded-lg border bg-muted p-0.5">
			{MODES.map(({ id, icon: Icon }) => (
				<button
					key={id}
					type="button"
					aria-label={t(`reader.${id}`)}
					aria-pressed={mode === id}
					onClick={() => onChange(id)}
					className={cn(
						"grid size-9 place-items-center rounded-md",
						mode === id && "bg-background shadow-raised",
					)}
				>
					<Icon className="size-4" />
				</button>
			))}
		</div>
	);
}

export function MobileAgentToolbar({
	agents,
	loading,
	selectedAgentId,
	remoteSelected,
	onSelectAgent,
	onSelectBridge,
	onSelectRemote,
	onOpenHistory,
}: {
	agents: AgentDescriptor[];
	loading: boolean;
	selectedAgentId: string | null;
	remoteSelected: boolean;
	onSelectAgent: (agentId: string) => void;
	onSelectBridge: () => void;
	onSelectRemote: () => void;
	onOpenHistory: () => void;
}) {
	const { t } = useTranslation(["mobile", "agent"]);
	const selectedAgent = agents.find((agent) => agent.id === selectedAgentId);
	const backendLabel = remoteSelected
		? "dsh-remote"
		: (selectedAgent?.name ?? t("agent.defaultBackend"));
	return (
		<div className="flex min-w-0 items-center gap-1">
			<DropdownMenu>
				<DropdownMenuTrigger asChild>
					<Button
						type="button"
						variant="outline"
						size="sm"
						className="h-11 min-w-0 max-w-[min(48vw,12rem)] justify-between gap-1.5 rounded-lg px-2.5"
						title={t("agent.switchBackend")}
					>
						<span className="truncate">{backendLabel}</span>
						<ChevronDown className="size-4 shrink-0 text-muted-foreground" />
					</Button>
				</DropdownMenuTrigger>
				<DropdownMenuContent align="start" className="w-56">
					<DropdownMenuRadioGroup
						value={remoteSelected ? "remote" : `acp:${selectedAgentId ?? ""}`}
					>
						<DropdownMenuLabel>{t("agent:agentMenu.title")}</DropdownMenuLabel>
						{remoteSelected && !agents.some((agent) => agent.available) ? (
							<DropdownMenuRadioItem
								value="bridge"
								onSelect={onSelectBridge}
								className="min-h-11"
							>
								{t("remote.bridgeProvider")}
							</DropdownMenuRadioItem>
						) : null}
						{agents.map((agent) => (
							<DropdownMenuRadioItem
								key={agent.id}
								value={`acp:${agent.id}`}
								disabled={loading || !agent.available}
								onSelect={() => {
									if (remoteSelected || agent.id !== selectedAgentId) {
										onSelectAgent(agent.id);
									}
								}}
								className="min-h-11"
							>
								<span className="min-w-0 truncate">{agent.name}</span>
							</DropdownMenuRadioItem>
						))}
						<DropdownMenuSeparator />
						<DropdownMenuLabel>{t("agent:agentMenu.remote")}</DropdownMenuLabel>
						<DropdownMenuRadioItem
							value="remote"
							onSelect={() => {
								if (!remoteSelected) onSelectRemote();
							}}
							className="min-h-11"
						>
							dsh-remote
						</DropdownMenuRadioItem>
					</DropdownMenuRadioGroup>
				</DropdownMenuContent>
			</DropdownMenu>
			{!remoteSelected ? (
				<Button
					type="button"
					variant="ghost"
					size="icon-sm"
					className="size-11"
					aria-label={t("agent.history")}
					onClick={onOpenHistory}
				>
					<History className="size-4" />
				</Button>
			) : null}
		</div>
	);
}
