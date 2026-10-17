import { Menu } from "@mantine/core";
import { IconArrowBarToRight, IconDots } from "@tabler/icons-react";
import { createContext, useContext, type ReactNode } from "react";
import { createPortal } from "react-dom";
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
