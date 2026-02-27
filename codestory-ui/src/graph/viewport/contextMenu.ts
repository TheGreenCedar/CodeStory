export type ContextMenuState =
  | {
      x: number;
      y: number;
      kind: "pane";
    }
  | {
      x: number;
      y: number;
      kind: "node";
      nodeId: string;
      label: string;
      filePath: string | null;
      isFile: boolean;
      isGroup: boolean;
      groupAnchorId: string | null;
    }
  | {
      x: number;
      y: number;
      kind: "edge";
      edgeId: string;
    };

export type ContextMenuPayload =
  | {
      kind: "pane";
    }
  | {
      kind: "node";
      nodeId: string;
      label: string;
      filePath: string | null;
      isFile: boolean;
      isGroup: boolean;
      groupAnchorId: string | null;
    }
  | {
      kind: "edge";
      edgeId: string;
    };

export function contextMenuPosition(
  clientX: number,
  clientY: number,
  shellRect: DOMRect | null | undefined,
): { x: number; y: number } {
  const x = shellRect ? clientX - shellRect.left : clientX;
  const y = shellRect ? clientY - shellRect.top : clientY;
  return { x, y };
}
