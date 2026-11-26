import { readFileSync } from "node:fs";
import { PdfiumNative } from "@embedpdf/engines/pdfium";
import { init } from "@embedpdf/pdfium";

export async function createNativePdfium() {
	return new PdfiumNative(
		await init({
			// WASI requires an ASCII name, independent of the checkout path
			// (mobile Unicode-workspace fix).
			thisProgram: "pdfium-native-test",
			wasmBinary: readFileSync(
				new URL(import.meta.resolve("@embedpdf/pdfium/pdfium.wasm")),
			),
		}),
		{ fontFallback: null },
	);
}
