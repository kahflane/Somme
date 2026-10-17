import { UnstyledButton } from "@mantine/core";
import {
  IconArrowBigRightLinesFilled,
  IconBraces,
  IconClockFilled,
  IconFingerprint,
  IconSettingsFilled,
} from "@tabler/icons-react";
import type { ComponentType } from "react";
import { useNavigation, type Section } from "@/application/stores/navigation";
import { VAULT_TAB_ID, useTabs } from "@/application/stores/tabs";
import { KeyGlyph, ServerGlyph } from "@/presentation/common/glyphs";
import classes from "@/presentation/shell/shell.module.css";

interface NavItem {
  section: Section;
  label: string;
  icon: ComponentType<{ size?: number; stroke?: number }>;
}

export const navItems: NavItem[] = [
  { section: "hosts", label: "Hosts", icon: ServerGlyph },
  { section: "keychain", label: "Keychain", icon: KeyGlyph },
  { section: "forwards", label: "Port Forwarding", icon: IconArrowBigRightLinesFilled },
  { section: "snippets", label: "Snippets", icon: IconBraces },
  { section: "knownHosts", label: "Known Hosts", icon: IconFingerprint },
  { section: "logs", label: "Logs", icon: IconClockFilled },
];

export const settingsItem: NavItem = { section: "settings", label: "Settings", icon: IconSettingsFilled };

const strokes: Partial<Record<Section, number>> = { snippets: 2.6, knownHosts: 2.2 };

export function Nav() {
  const section = useNavigation((state) => state.section);
  const setSection = useNavigation((state) => state.setSection);
  const activate = useTabs((state) => state.activate);

  const renderItem = (item: NavItem) => {
    const active = section === item.section;
    return (
      <UnstyledButton
        key={item.section}
        className={`${classes.navItem} ${active ? classes.navItemActive : ""}`}
        aria-current={active ? "page" : undefined}
        onClick={() => {
          setSection(item.section);
          activate(VAULT_TAB_ID);
        }}
      >
        <span className={classes.navIcon}>
          <item.icon size={18} stroke={strokes[item.section] ?? 2} />
        </span>
        <span>{item.label}</span>
      </UnstyledButton>
    );
  };

  return (
    <nav className={classes.nav}>
      {navItems.map(renderItem)}
      <div className={classes.navFooter}>{renderItem(settingsItem)}</div>
    </nav>
  );
}
