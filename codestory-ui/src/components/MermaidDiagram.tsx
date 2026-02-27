import { useEffect, useState } from "react";
import mermaid from "mermaid";

let mermaidInitialized = false;

function ensureMermaidInitialized() {
  if (mermaidInitialized) {
    return;
  }
  mermaid.initialize({
    startOnLoad: false,
    theme: "neutral",
    securityLevel: "loose",
  });
  mermaidInitialized = true;
}

type MermaidDiagramProps = {
  syntax: string;
  loadingMessage?: string;
  className?: string;
};

export function MermaidDiagram({
  syntax,
  loadingMessage = "Rendering diagram...",
  className = "mermaid-shell",
}: MermaidDiagramProps) {
  const [svg, setSvg] = useState<string>("");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    ensureMermaidInitialized();
    let disposed = false;
    const renderId = `mermaid-${Math.random().toString(36).slice(2)}`;

    mermaid
      .render(renderId, syntax)
      .then(({ svg: renderedSvg }) => {
        if (!disposed) {
          setSvg(renderedSvg);
          setError(null);
        }
      })
      .catch((err: unknown) => {
        if (!disposed) {
          setError(err instanceof Error ? err.message : "Failed to render Mermaid diagram.");
          setSvg("");
        }
      });

    return () => {
      disposed = true;
    };
  }, [syntax]);

  if (error) {
    return <div className="graph-empty">{error}</div>;
  }

  if (svg.length === 0) {
    return <div className="graph-empty">{loadingMessage}</div>;
  }

  return <div className={className} dangerouslySetInnerHTML={{ __html: svg }} />;
}
