import type {
	AnnotationEvent,
	AnnotationScope,
	AnnotationTransferItem,
} from "@embedpdf/plugin-annotation";
import { isLiveAnnotation } from "@/lib/pdf/highlight/annotation-store";
import { normalizePathKey } from "@/lib/vault/path";

type Mutation = Exclude<AnnotationEvent, { type: "loaded" }>;
type Peer = {
	documentId: string;
	epoch: number;
	receive: (event: Mutation) => void;
};
type PeerGroup = {
	peers: Set<Peer>;
	latest: Map<string, Mutation>;
	epoch: number;
	baseline?: string | null;
};
const groups = new Map<string, PeerGroup>();
const saves = new Map<string, Promise<void>>();
export function mergeAnnotationSnapshot(
	snapshot: AnnotationTransferItem[],
	disk: AnnotationTransferItem[],
	knownIds: ReadonlySet<string>,
	editedIds: ReadonlySet<string>,
): AnnotationTransferItem[] {
	const byId = new Map(disk.map((item) => [item.annotation.id, item]));
	const preserved = snapshot.flatMap((item) => {
		const id = item.annotation.id;
		if (editedIds.has(id) || !knownIds.has(id)) return [item];
		const latest = byId.get(id);
		return latest ? [latest] : [];
	});
	const savedIds = new Set(preserved.map((item) => item.annotation.id));
	return preserved.concat(
		disk.filter(
			(item) =>
				!editedIds.has(item.annotation.id) && !savedIds.has(item.annotation.id),
		),
	);
}

/** Mirror user edits between independent PDF engines, without watcher echo or re-saving imports. */
export function subscribeAnnotationPeer(
	paperPath: string,
	documentId: string,
	receive: Peer["receive"],
	baseline?: string | null,
): () => void {
	const key = normalizePathKey(paperPath);
	let group = groups.get(key);
	if (!group) {
		group = { peers: new Set(), latest: new Map(), epoch: 0, baseline };
		groups.set(key, group);
	} else if (baseline !== undefined) {
		invalidateAnnotationPeers(paperPath, baseline);
	}
	const peer = { documentId, receive, epoch: group.epoch };
	group.peers.add(peer);
	// A new embed can open before the writer's debounced sidecar save finishes.
	for (const event of group.latest.values()) receive(event);
	return () => {
		group.peers.delete(peer);
		if (!group.peers.size) groups.delete(key);
	};
}

/** Invalidate replay and old peers when an authoritative disk version changes. */
export function invalidateAnnotationPeers(
	paperPath: string,
	baseline: string | null,
): void {
	const group = groups.get(normalizePathKey(paperPath));
	if (!group || group.baseline === baseline) return;
	group.baseline = baseline;
	group.epoch += 1;
	group.latest.clear();
}

/** A successful guarded save advances the current epoch without replaying stale peers. */
export function advanceAnnotationPeerBaseline(
	paperPath: string,
	documentId: string,
	previous: string | null | undefined,
	next: string | null | undefined,
): void {
	const group = groups.get(normalizePathKey(paperPath));
	if (!group || group.baseline !== previous || next === undefined) return;
	const sender = [...group.peers].find(
		(peer) => peer.documentId === documentId,
	);
	if (sender?.epoch === group.epoch) group.baseline = next;
}

export function publishAnnotationMutation(
	paperPath: string,
	event: AnnotationEvent,
): void {
	if (event.type === "loaded" || event.committed) return;
	const group = groups.get(normalizePathKey(paperPath));
	if (!group) return;
	const sender = [...group.peers].find(
		(peer) => peer.documentId === event.documentId,
	);
	if (sender?.epoch !== group.epoch) return;
	group.latest.set(event.annotation.id, event);
	for (const peer of group.peers) {
		if (peer.epoch === group.epoch && peer.documentId !== event.documentId)
			peer.receive(event);
	}
}

export function applyAnnotationMutation(
	scope: AnnotationScope,
	event: Mutation,
): void {
	const current = scope.getAnnotationById(event.annotation.id);
	const live = current && isLiveAnnotation(current);
	if (event.type === "delete") {
		if (live) scope.deleteAnnotation(event.pageIndex, event.annotation.id);
	} else if (event.type === "update" && live) {
		scope.updateAnnotation(event.pageIndex, event.annotation.id, event.patch);
	} else if (!live) {
		scope.createAnnotation(
			event.pageIndex,
			event.annotation,
			event.type === "create" ? event.ctx : undefined,
		);
		if (event.type === "update")
			scope.updateAnnotation(event.pageIndex, event.annotation.id, event.patch);
	}
}

/** Save snapshots in request order, even when an earlier export takes longer. */
export async function enqueueAnnotationSave(
	paperPath: string,
	save: () => Promise<void>,
): Promise<void> {
	const key = normalizePathKey(paperPath);
	const next = (saves.get(key) ?? Promise.resolve()).catch(() => {}).then(save);
	saves.set(key, next);
	try {
		await next;
	} finally {
		if (saves.get(key) === next) saves.delete(key);
	}
}
