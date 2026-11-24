import { ChevronDown } from "lucide-react";
import { Fragment, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import {
	DropdownMenu,
	DropdownMenuContent,
	DropdownMenuLabel,
	DropdownMenuRadioGroup,
	DropdownMenuRadioItem,
	DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import type {
	RemoteModelSelection,
	RemoteSessionModels,
	SessionRef,
} from "@/lib/core/bindings";
import { errorText } from "@/lib/core/error";
import { notifyError } from "@/lib/core/notify";
import { cn } from "@/lib/core/utils";
import { remoteAgent } from "@/lib/remote-agent/service";

function modelKey(provider: string, model: string) {
	return JSON.stringify([provider, model]);
}

/** Mounted per session so a late catalog response cannot replace another session's model. */
export function RemoteModelMenu({
	session,
	mobile,
	disabled,
	onSelect,
}: {
	session: SessionRef;
	mobile: boolean;
	disabled: boolean;
	onSelect: (
		selection: RemoteModelSelection,
	) => Promise<RemoteModelSelection | null>;
}) {
	const { t } = useTranslation("agent");
	const [catalog, setCatalog] = useState<RemoteSessionModels | null>();
	const [loading, setLoading] = useState(false);
	const current = catalog?.current;
	const modelLabel = current
		? (catalog?.groups
				.find((group) => group.id === current.provider)
				?.models.find((model) => model.id === current.model)?.name ??
			current.model)
		: t("models.button");

	async function loadModels() {
		if (loading || disabled) return;
		setLoading(true);
		try {
			setCatalog(await remoteAgent.sessionModels(session));
		} catch (error) {
			setCatalog((previous) => previous ?? null);
			notifyError(errorText(error));
		} finally {
			setLoading(false);
		}
	}

	return (
		<DropdownMenu
			onOpenChange={(open) => {
				if (open) void loadModels();
			}}
		>
			<DropdownMenuTrigger asChild disabled={disabled}>
				<Button
					type="button"
					size="sm"
					variant="ghost"
					aria-label={t("models.button")}
					title={modelLabel}
					className={cn(
						"me-auto min-w-0 max-w-full shrink gap-1 px-2",
						mobile ? "h-11" : "h-8",
					)}
				>
					<span className="truncate">{modelLabel}</span>
					<ChevronDown className="size-3.5 shrink-0 opacity-70" />
				</Button>
			</DropdownMenuTrigger>
			<DropdownMenuContent
				align="start"
				side="top"
				className="max-h-72 w-56 overflow-y-auto"
			>
				{!loading &&
				catalog?.groups.some((group) => group.models.length > 0) ? (
					<DropdownMenuRadioGroup
						value={
							current ? modelKey(current.provider, current.model) : undefined
						}
					>
						{catalog.groups
							.filter((group) => group.models.length > 0)
							.map((group) => (
								<Fragment key={group.id}>
									<DropdownMenuLabel>{group.name}</DropdownMenuLabel>
									{group.models.map((model) => (
										<DropdownMenuRadioItem
											key={model.id}
											value={modelKey(group.id, model.id)}
											disabled={disabled}
											className={mobile ? "min-h-11" : undefined}
											title={model.description ?? undefined}
											onSelect={() => {
												if (
													current?.provider === group.id &&
													current.model === model.id
												)
													return;
												void onSelect({
													provider: group.id,
													model: model.id,
												}).then((selected) => {
													if (selected)
														setCatalog({ ...catalog, current: selected });
												});
											}}
										>
											<span className="truncate">{model.name}</span>
										</DropdownMenuRadioItem>
									))}
								</Fragment>
							))}
					</DropdownMenuRadioGroup>
				) : (
					<p className="px-2 py-1.5 text-muted-foreground text-sm">
						{t(
							loading || catalog === undefined
								? "models.loading"
								: "models.emptyCatalog",
						)}
					</p>
				)}
			</DropdownMenuContent>
		</DropdownMenu>
	);
}
