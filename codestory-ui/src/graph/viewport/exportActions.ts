import { useCallback, type RefObject } from "react";
import { toBlob, toJpeg, toPng, toSvg } from "html-to-image";

import { GRAPH_THEME } from "../../theme/tokens";

function graphExportBaseName(graphTitle: string): string {
  const raw = graphTitle
    .trim()
    .replace(/\s+/g, "_")
    .replace(/[^a-zA-Z0-9_.-]/g, "");
  return raw.length > 0 ? raw : "graph";
}

type UseGraphExportActionsArgs = {
  graphTitle: string;
  flowShellRef: RefObject<HTMLDivElement | null>;
  onStatusMessage?: (message: string) => void;
};

export type GraphExportActions = {
  exportImage: (format: "png" | "jpeg" | "svg") => Promise<void>;
  exportToClipboard: () => Promise<void>;
  copyText: (text: string, successMessage: string) => Promise<void>;
};

export function useGraphExportActions({
  graphTitle,
  flowShellRef,
  onStatusMessage,
}: UseGraphExportActionsArgs): GraphExportActions {
  const exportRootElement = useCallback((): HTMLElement | null => {
    const shell = flowShellRef.current;
    if (!shell) {
      return null;
    }
    const viewport = shell.querySelector<HTMLElement>(".react-flow__viewport");
    if (viewport) {
      return viewport;
    }
    return shell.querySelector<HTMLElement>(".react-flow");
  }, [flowShellRef]);

  const triggerDownload = useCallback((fileName: string, dataUrl: string) => {
    const anchor = document.createElement("a");
    anchor.href = dataUrl;
    anchor.download = fileName;
    document.body.append(anchor);
    anchor.click();
    anchor.remove();
  }, []);

  const exportImage = useCallback(
    async (format: "png" | "jpeg" | "svg") => {
      const element = exportRootElement();
      if (!element) {
        onStatusMessage?.("Unable to capture graph image right now.");
        return;
      }
      const baseName = graphExportBaseName(graphTitle);
      const options = {
        cacheBust: true,
        backgroundColor: GRAPH_THEME.exportBackground,
        pixelRatio: 2,
      };
      try {
        if (format === "png") {
          const dataUrl = await toPng(element, options);
          triggerDownload(`${baseName}.png`, dataUrl);
          onStatusMessage?.("PNG export saved.");
          return;
        }
        if (format === "jpeg") {
          const dataUrl = await toJpeg(element, { ...options, quality: 0.96 });
          triggerDownload(`${baseName}.jpg`, dataUrl);
          onStatusMessage?.("JPEG export saved.");
          return;
        }
        const dataUrl = await toSvg(element, options);
        triggerDownload(`${baseName}.svg`, dataUrl);
        onStatusMessage?.("SVG export saved.");
      } catch (error) {
        onStatusMessage?.(
          error instanceof Error ? `Image export failed: ${error.message}` : "Image export failed.",
        );
      }
    },
    [exportRootElement, graphTitle, onStatusMessage, triggerDownload],
  );

  const exportToClipboard = useCallback(async () => {
    const element = exportRootElement();
    if (!element) {
      onStatusMessage?.("Unable to copy graph image right now.");
      return;
    }
    if (
      typeof navigator === "undefined" ||
      !navigator.clipboard ||
      typeof ClipboardItem === "undefined"
    ) {
      onStatusMessage?.("Clipboard image export is not supported in this browser context.");
      return;
    }
    try {
      const blob = await toBlob(element, {
        cacheBust: true,
        backgroundColor: GRAPH_THEME.exportBackground,
        pixelRatio: 2,
      });
      if (!blob) {
        onStatusMessage?.("Clipboard export failed: empty image payload.");
        return;
      }
      await navigator.clipboard.write([new ClipboardItem({ [blob.type]: blob })]);
      onStatusMessage?.("Graph copied to clipboard as PNG.");
    } catch (error) {
      onStatusMessage?.(
        error instanceof Error
          ? `Clipboard export failed: ${error.message}`
          : "Clipboard export failed.",
      );
    }
  }, [exportRootElement, onStatusMessage]);

  const copyText = useCallback(
    async (text: string, successMessage: string) => {
      if (!navigator.clipboard) {
        onStatusMessage?.("Clipboard is unavailable in this context.");
        return;
      }
      try {
        await navigator.clipboard.writeText(text);
        onStatusMessage?.(successMessage);
      } catch (error) {
        onStatusMessage?.(
          error instanceof Error ? `Copy failed: ${error.message}` : "Copy failed.",
        );
      }
    },
    [onStatusMessage],
  );

  return {
    exportImage,
    exportToClipboard,
    copyText,
  };
}
