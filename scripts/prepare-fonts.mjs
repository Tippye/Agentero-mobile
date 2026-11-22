/**
 * Stage the bundled reflow fallback font. No font download at runtime.
 *
 * The reflow engine needs embedded fonts to typeset a translated PDF. On
 * desktop the six families are fetched once from the pinned public assets and
 * cached; on a phone that host is often unreachable, and typesetting then fails
 * with "Failed to fetch". One CJK serif ships inside the APK instead: it covers
 * the Latin glyphs too, so it can stand in for every family when the download
 * cannot run. Bold and italic shapes are not synthesized, which is acceptable
 * for a fallback.
 */
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const out = path.join(root, "src-tauri/fonts");
// Mirrors the pinned revision in src/lib/pdf/reflow/fonts.ts.
const revision = "b956e9333a4e53ed9753c296877af6c7074d7b23";
const name = "SourceHanSerifCN-Regular.ttf";
const sha256 =
	"8ba5ec09db04b1d1599edeff3fb5627ca11eaaf85e339e5c32684cb94e806993";
const output = path.join(out, name);
const stamp = path.join(out, "version.txt");
const hash = (bytes) => createHash("sha256").update(bytes).digest("hex");
const version = `${revision}:${name}:${sha256}`;
const sources = [
	`https://www.modelscope.cn/datasets/awwaawwa/BabelDOCAssets/resolve/${revision}/fonts/${name}`,
	`https://raw.githubusercontent.com/funstory-ai/BabelDOC-Assets/${revision}/fonts/${name}`,
	`https://cdn.jsdelivr.net/gh/funstory-ai/BabelDOC-Assets@${revision}/fonts/${name}`,
];

function verify() {
	if (!fs.existsSync(output)) return false;
	const bytes = fs.readFileSync(output);
	if (hash(bytes) !== sha256) return false;
	if (!fs.existsSync(stamp)) return false;
	return fs.readFileSync(stamp, "utf8").trim() === version;
}

async function download() {
	fs.mkdirSync(out, { recursive: true });
	for (const url of sources) {
		process.stdout.write(`[fonts] fetching ${url}\n`);
		try {
			const response = await fetch(url, {
				signal: AbortSignal.timeout(180_000),
			});
			if (!response.ok) {
				process.stdout.write(`[fonts] ${response.status} from ${url}\n`);
				continue;
			}
			const bytes = new Uint8Array(await response.arrayBuffer());
			if (hash(bytes) !== sha256) {
				process.stdout.write(`[fonts] checksum mismatch from ${url}\n`);
				continue;
			}
			fs.writeFileSync(output, bytes);
			fs.writeFileSync(stamp, `${version}\n`);
			return true;
		} catch (error) {
			process.stdout.write(
				`[fonts] ${error instanceof Error ? error.message : error}\n`,
			);
		}
	}
	return false;
}

if (!verify()) {
	if (!(await download()))
		throw new Error(
			"[fonts] could not stage the fallback font; set a proxy or place " +
				`${output} manually`,
		);
}

const bytes = fs.statSync(output).size;
console.log(
	`[fonts] verified ${name} (${(bytes / 1048576).toFixed(1)} MB) @ ${revision.slice(0, 8)}`,
);
