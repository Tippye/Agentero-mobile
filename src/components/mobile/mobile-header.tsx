import { Circle } from "lucide-react";
import type { ReactNode } from "react";
import type { BridgeClientStatus } from "@/lib/bridge/client";
import { cn } from "@/lib/core/utils";

export function MobileHeader({
	title,
	status,
	statusLabel,
	brand,
	brandButtonLabel,
	onBrandClick,
	showBrand = true,
	showStatus = true,
	wide = false,
	leading,
	trailing,
}: {
	title: string;
	status: BridgeClientStatus;
	statusLabel: string;
	brand: ReactNode;
	brandButtonLabel?: string;
	onBrandClick?: () => void;
	showBrand?: boolean;
	showStatus?: boolean;
	wide?: boolean;
	leading?: ReactNode;
	trailing?: ReactNode;
}) {
	return (
		<header
			className={cn(
				"relative z-30 flex min-h-16 shrink-0 select-none items-center gap-3 border-b bg-background px-4",
				wide && "px-6",
			)}
		>
			{leading}
			<div className="flex min-w-0 flex-1 items-center gap-2">
				<div className={cn((wide || !showBrand) && "hidden")}>
					{onBrandClick ? (
						<button
							type="button"
							className="grid size-11 place-items-center rounded-md outline-none transition-opacity focus-visible:ring-1 focus-visible:ring-ring active:opacity-70"
							aria-label={brandButtonLabel}
							onClick={onBrandClick}
						>
							{brand}
						</button>
					) : (
						brand
					)}
				</div>
				<span className="truncate font-semibold text-base">{title}</span>
				{showStatus ? (
					<Circle
						className={cn(
							"size-2.5 shrink-0 fill-current",
							status.connected ? "text-emerald-500" : "text-muted-foreground",
						)}
						aria-label={statusLabel}
					/>
				) : null}
			</div>
			{trailing}
		</header>
	);
}
