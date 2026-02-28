import type { HTMLAttributes } from "react";

import { cx } from "./cx";

type BadgeTone = "default" | "accent" | "secondary" | "tertiary" | "quaternary";

type BadgeProps = HTMLAttributes<HTMLSpanElement> & {
  tone?: BadgeTone;
};

export function Badge({ tone = "default", className, ...props }: BadgeProps) {
  return (
    <span
      className={cx(
        "ui-badge",
        tone === "accent" ? "ui-badge-accent" : "",
        tone === "secondary" ? "ui-badge-secondary" : "",
        tone === "tertiary" ? "ui-badge-tertiary" : "",
        tone === "quaternary" ? "ui-badge-quaternary" : "",
        className,
      )}
      {...props}
    />
  );
}
