import { Menu, TextInput, Tooltip } from "@mantine/core";
import {
  IconCalendarMonthFilled,
  IconChevronDown,
  IconLayoutGridFilled,
  IconLayoutListFilled,
  IconPlus,
  IconSearch,
} from "@tabler/icons-react";
import { useState, type KeyboardEvent, type MouseEvent, type ReactNode } from "react";
import { useSortOrder, useViewMode, type SortOrder, type ViewMode } from "@/application/stores/view-modes";
import classes from "@/presentation/common/page.module.css";

interface PageProps {
  search?: ReactNode;
  toolbar?: ReactNode;
  tools?: ReactNode;
  fill?: boolean;
  children: ReactNode;
}

export function Page({ search, toolbar, tools, fill, children }: PageProps) {
  return (
    <div className={classes.page}>
      <div className={classes.header}>
        {search && <div className={classes.search}>{search}</div>}
        <div className={classes.toolbar}>
          <div className={classes.toolbarStart}>{toolbar}</div>
          {tools && <div className={classes.toolbarEnd}>{tools}</div>}
        </div>
      </div>
      <div className={`${classes.body} ${fill ? classes.bodyFill : ""}`}>{children}</div>
    </div>
  );
}

export function PageSection({ title, children }: { title: ReactNode; children: ReactNode }) {
  return (
    <section className={classes.section}>
      <h2 className={classes.sectionTitle}>{title}</h2>
      {children}
    </section>
  );
}

export function CardGrid({ view, children }: { view: ViewMode; children: ReactNode }) {
  return (
    <div className={classes.grid} data-view={view}>
      {children}
    </div>
  );
}

export function IconTile({ color, children }: { color?: string | null; children: ReactNode }) {
  return (
    <div className={classes.tile} style={color ? { background: color } : undefined}>
      {children}
    </div>
  );
}

interface EntityCardProps {
  icon: ReactNode;
  tileColor?: string | null;
  title: ReactNode;
  subtitle?: ReactNode;
  active?: boolean;
  badge?: boolean;
  action?: { label: string; icon: ReactNode; onClick: () => void };
  onClick?: () => void;
  onContextMenu?: (event: MouseEvent) => void;
}

export function EntityCard({
  icon,
  tileColor,
  title,
  subtitle,
  active,
  badge,
  action,
  onClick,
  onContextMenu,
}: EntityCardProps) {
  const onKeyDown = (event: KeyboardEvent) => {
    if (event.target !== event.currentTarget) return;
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      onClick?.();
    }
  };
  return (
    <div
      role="button"
      tabIndex={0}
      className={classes.card}
      data-active={active || undefined}
      onClick={onClick}
      onKeyDown={onKeyDown}
      onContextMenu={onContextMenu}
    >
      <IconTile color={tileColor}>{icon}</IconTile>
      <div className={classes.cardText}>
        <span className={classes.cardTitle}>{title}</span>
        {subtitle && <span className={classes.cardSubtitle}>{subtitle}</span>}
      </div>
      {badge && <span className={classes.cardBadge} />}
      {action && (
        <Tooltip label={action.label}>
          <button
            type="button"
            className={classes.cardAction}
            aria-label={action.label}
            onClick={(event) => {
              event.stopPropagation();
              action.onClick();
            }}
          >
            {action.icon}
          </button>
        </Tooltip>
      )}
    </div>
  );
}

interface ToolButtonProps {
  icon?: ReactNode;
  filled?: boolean;
  active?: boolean;
  onClick?: () => void;
  children: ReactNode;
}

export function ToolButton({ icon, filled, active, onClick, children }: ToolButtonProps) {
  return (
    <button
      type="button"
      className={classes.toolButton}
      data-filled={filled || undefined}
      data-active={active || undefined}
      onClick={onClick}
    >
      {icon}
      {children}
    </button>
  );
}

interface SplitButtonProps {
  label: string;
  onClick: () => void;
  menu?: ReactNode;
}

export function SplitButton({ label, onClick, menu }: SplitButtonProps) {
  const main = (
    <ToolButton filled icon={<IconPlus size={16} stroke={2.4} />} onClick={onClick}>
      {label}
    </ToolButton>
  );
  if (!menu) return main;
  return (
    <div className={classes.split}>
      {main}
      <Menu position="bottom-start">
        <Menu.Target>
          <button
            type="button"
            className={`${classes.toolButton} ${classes.splitChevron}`}
            data-filled
            aria-label={`More ${label.toLowerCase()} options`}
          >
            <IconChevronDown size={16} stroke={2.2} />
          </button>
        </Menu.Target>
        <Menu.Dropdown>{menu}</Menu.Dropdown>
      </Menu>
    </div>
  );
}

interface IconButtonProps {
  label: string;
  active?: boolean;
  dimmed?: boolean;
  onClick?: () => void;
  children: ReactNode;
}

export function IconButton({ label, active, dimmed, onClick, children }: IconButtonProps) {
  return (
    <Tooltip label={label}>
      <button
        type="button"
        className={classes.iconButton}
        aria-label={label}
        aria-pressed={active}
        data-active={active || undefined}
        data-dimmed={dimmed || undefined}
        onClick={onClick}
      >
        {children}
      </button>
    </Tooltip>
  );
}

export function SearchToggle({ query, onQuery }: { query: string; onQuery: (value: string) => void }) {
  const [open, setOpen] = useState(false);
  const close = () => {
    setOpen(false);
    onQuery("");
  };
  if (open || query) {
    return (
      <TextInput
        className={classes.searchField}
        autoFocus
        placeholder="Search"
        leftSection={<IconSearch size={14} />}
        value={query}
        onChange={(event) => onQuery(event.currentTarget.value)}
        onBlur={() => {
          if (!query) setOpen(false);
        }}
        onKeyDown={(event) => {
          if (event.key === "Escape") close();
        }}
      />
    );
  }
  return (
    <IconButton label="Search" onClick={() => setOpen(true)}>
      <IconSearch size={18} stroke={2.2} />
    </IconButton>
  );
}

export function ViewToggle({ page }: { page: string }) {
  const [view, setView] = useViewMode(page);
  const next = view === "grid" ? "list" : "grid";
  return (
    <IconButton label={next === "list" ? "Show as list" : "Show as grid"} onClick={() => setView(next)}>
      {view === "grid" ? <IconLayoutGridFilled size={18} /> : <IconLayoutListFilled size={18} />}
    </IconButton>
  );
}

const sortLabels: { value: SortOrder; label: string }[] = [
  { value: "az", label: "A-z" },
  { value: "za", label: "Z-a" },
  { value: "newest", label: "Newest to oldest" },
  { value: "oldest", label: "Oldest to newest" },
];

export function SortMenu({ page }: { page: string }) {
  const [sort, setSort] = useSortOrder(page);
  return (
    <Menu position="bottom-end" width={190}>
      <Menu.Target>
        <span>
          <IconButton label="Sort">
            <IconCalendarMonthFilled size={18} />
          </IconButton>
        </span>
      </Menu.Target>
      <Menu.Dropdown>
        <Menu.Label>Sort by</Menu.Label>
        {sortLabels.map((item) => (
          <Menu.Item
            key={item.value}
            rightSection={sort === item.value ? "✓" : null}
            onClick={() => setSort(item.value)}
          >
            {item.label}
          </Menu.Item>
        ))}
      </Menu.Dropdown>
    </Menu>
  );
}

export function ToolbarTitle({ children }: { children: ReactNode }) {
  return <span className={classes.toolbarTitle}>{children}</span>;
}

export function Muted({ children }: { children: ReactNode }) {
  return <p className={classes.muted}>{children}</p>;
}
