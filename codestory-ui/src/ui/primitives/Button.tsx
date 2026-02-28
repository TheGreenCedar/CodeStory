import type { ButtonHTMLAttributes, ReactNode } from "react";

import { cx } from "./cx";

type ButtonVariant = "primary" | "secondary" | "ghost";

type ButtonProps = ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: ButtonVariant;
  leadingIcon?: ReactNode;
  trailingIcon?: ReactNode;
};

export function Button({
  variant = "secondary",
  leadingIcon,
  trailingIcon,
  className,
  children,
  type = "button",
  ...props
}: ButtonProps) {
  return (
    <button
      type={type}
      className={cx(
        "ui-button",
        variant === "primary" ? "ui-button-primary" : "",
        variant === "secondary" ? "ui-button-secondary" : "",
        variant === "ghost" ? "ui-button-ghost" : "",
        className,
      )}
      {...props}
    >
      {leadingIcon}
      {children}
      {trailingIcon}
    </button>
  );
}
