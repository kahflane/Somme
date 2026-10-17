import { Menu } from "@mantine/core";
import { IconArrowBarToRight, IconChevronDown, IconDots } from "@tabler/icons-react";
import { createContext, useContext, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { VaultGlyph } from "@/presentation/common/glyphs";
import { IconButton } from "@/presentation/common/page";
import classes from "@/presentation/common/side-panel.module.css";

export const SidePanelSlotContext = createContext<HTMLElement | null>(null);

export const sidePanelSlotClass = classes.slot;

interface Props {
  opened: boolean;
  title: ReactNode;
  onClose: () => void;
  menu?: ReactNode;
  footer?: ReactNode;
  flush?: boolean;
  children: ReactNode;
}

export function SidePanel({ opened, title, onClose, menu, footer, flush, children }: Props) {
  const slot = useContext(SidePanelSlotContext);
  if (!opened || !slot) return null;
  return createPortal(
    <aside
      className={classes.panel}
      onKeyDown={(event) => {
        if (event.key === "Escape" && !event.defaultPrevented) onClose();
      }}
    >
      <header className={classes.header}>
        <span className={classes.title}>{title}</span>
        {menu && (
          <Menu position="bottom-end">
            <Menu.Target>
              <span>
                <IconButton label="More">
                  <IconDots size={20} stroke={2.4} />
                </IconButton>
              </span>
            </Menu.Target>
            <Menu.Dropdown>{menu}</Menu.Dropdown>
          </Menu>
        )}
        <IconButton label="Close panel" onClick={onClose}>
          <IconArrowBarToRight size={20} stroke={2.2} />
        </IconButton>
      </header>
      <div className={`${classes.body} ${flush ? classes.flush : ""}`}>{children}</div>
      {footer && <div className={classes.footer}>{footer}</div>}
    </aside>,
    slot,
  );
}

export function PanelSection({ title, children }: { title?: string; children: ReactNode }) {
  return (
    <section className={classes.section}>
      {title && <h3 className={classes.sectionTitle}>{title}</h3>}
      <div className={classes.sectionBody}>{children}</div>
    </section>
  );
}

export function PanelCard({ children }: { children: ReactNode }) {
  return <div className={classes.card}>{children}</div>;
}

// Somme keeps everything in one vault; the picker mirrors Termius and shows it.
export function VaultPicker() {
  return (
    <Menu position="bottom" width="target">
      <Menu.Target>
        <button type="button" className={classes.select}>
          <VaultGlyph size={18} />
          <span className={classes.selectLabel}>Personal</span>
          <IconChevronDown size={18} stroke={2.4} />
        </button>
      </Menu.Target>
      <Menu.Dropdown>
        <Menu.Item rightSection="✓">Personal</Menu.Item>
      </Menu.Dropdown>
    </Menu>
  );
}

interface PrimaryProps {
  onClick?: () => void;
  disabled?: boolean;
  type?: "button" | "submit";
  children: ReactNode;
}

export function PanelPrimaryButton({ onClick, disabled, type = "button", children }: PrimaryProps) {
  return (
    <button type={type} className={classes.primary} disabled={disabled} onClick={onClick}>
      {children}
    </button>
  );
}
