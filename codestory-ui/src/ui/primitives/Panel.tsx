import { createElement, type ElementType, type HTMLAttributes } from "react";

import { cx } from "./cx";

type PanelElement = "section" | "div" | "article";

type PanelProps<T extends PanelElement = "section"> = HTMLAttributes<HTMLElement> & {
  as?: T;
};

export function Panel({ as = "section", className, ...props }: PanelProps) {
  return createElement(as as ElementType, {
    className: cx("ui-panel", className),
    ...props,
  });
}
