import { createElement, type ElementType, type HTMLAttributes } from "react";

import { cx } from "./cx";

type CardTone = "default" | "accent" | "secondary" | "tertiary";
type CardElement = "div" | "section" | "article";

type CardProps = HTMLAttributes<HTMLElement> & {
  as?: CardElement;
  tone?: CardTone;
};

export function Card({ as = "div", tone = "default", className, ...props }: CardProps) {
  return createElement(as as ElementType, {
    className: cx(
      "ui-card",
      tone === "accent" ? "ui-card-accent" : "",
      tone === "secondary" ? "ui-card-secondary" : "",
      tone === "tertiary" ? "ui-card-tertiary" : "",
      className,
    ),
    ...props,
  });
}
