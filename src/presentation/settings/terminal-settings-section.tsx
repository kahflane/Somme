import {
  ActionIcon,
  Button,
  Group,
  NumberInput,
  SegmentedControl,
  Select,
  SimpleGrid,
  Slider,
  Stack,
  Switch,
  Text,
  Tooltip,
  UnstyledButton,
} from "@mantine/core";
import { IconCheck, IconRefresh, IconRestore } from "@tabler/icons-react";
import { useEffect, useMemo } from "react";
import { useRescanFonts, useSystemFonts } from "@/application/hooks/system-fonts";
import { useTerminalSettings, type CursorStyle } from "@/application/stores/terminal-settings";
import { SettingsSection } from "@/presentation/settings/settings-section";
import classes from "@/presentation/settings/settings.module.css";
import { findFont, fontStack, loadFont, systemFontId, terminalFonts } from "@/presentation/terminal/fonts";
import { useIconFont } from "@/presentation/terminal/use-icon-font";
import { findScheme, terminalSchemes, type TerminalScheme } from "@/presentation/terminal/schemes";

const cursorStyles: { value: CursorStyle; label: string }[] = [
  { value: "block", label: "Block" },
  { value: "bar", label: "Bar" },
  { value: "underline", label: "Underline" },
];

function SchemeSwatch({
  scheme,
  selected,
  onSelect,
}: {
  scheme: TerminalScheme;
  selected: boolean;
  onSelect: () => void;
}) {
  return (
    <UnstyledButton
      className={`${classes.swatch} ${selected ? classes.swatchSelected : ""}`}
      onClick={onSelect}
      aria-pressed={selected}
    >
      <div
        className={classes.swatchPreview}
        style={{ background: scheme.background, color: scheme.foreground }}
      >
        <span style={{ color: scheme.palette[2] }}>❯</span>{" "}
        <span style={{ color: scheme.palette[4] }}>ssh</span> web
        <div className={classes.swatchDots}>
          {scheme.palette.slice(1, 7).map((color, index) => (
            <span key={index} className={classes.swatchDot} style={{ background: color }} />
          ))}
        </div>
      </div>
      <Group justify="space-between" gap={4} px={8} py={6} wrap="nowrap">
        <Text size="xs" fw={700} truncate>
          {scheme.name}
        </Text>
        {selected && <IconCheck size={14} color="var(--x-accent)" />}
      </Group>
    </UnstyledButton>
  );
}

function Preview({ iconFont }: { iconFont: string | null }) {
  const settings = useTerminalSettings();
  const scheme = findScheme(settings.schemeId);
  const font = findFont(settings.fontId);

  useEffect(() => {
    loadFont(font, settings.fontSize, iconFont);
  }, [font.id, settings.fontSize, iconFont]);

  const [, red, green, yellow, blue, magenta, cyan] = scheme.palette;
  return (
    <div
      className={classes.preview}
      style={{
        background: scheme.background,
        color: scheme.foreground,
        fontFamily: fontStack(font, iconFont),
        fontSize: settings.fontSize,
        lineHeight: settings.lineHeight,
        letterSpacing: settings.letterSpacing,
      }}
    >
      <div>
        <span style={{ color: green }}>deploy@web-01</span>:<span style={{ color: blue }}>~/app</span>$ git
        status
      </div>
      <div>
        On branch <span style={{ color: cyan }}>main</span>
      </div>
      <div>
        <span style={{ color: red }}>modified:</span> src/server.rs
      </div>
      <div>
        <span style={{ color: yellow }}>warning:</span> 2 tests skipped{" "}
        <span style={{ color: magenta }}>→</span> cargo test
      </div>
      <div>
        <span style={{ color: blue }}>{"\uf31b \ue0b0"}</span>{" "}
        <span style={{ color: cyan }}>{"\ue725 main"}</span>{" "}
        <span style={{ color: yellow }}>{"\uf07c ~/app"}</span> 🚀
      </div>
      <div>
        <span style={{ color: green }}>deploy@web-01</span>:<span style={{ color: blue }}>~/app</span>${" "}
        <span
          className={classes.cursor}
          data-style={settings.cursorStyle}
          data-blink={settings.cursorBlink || undefined}
          style={{ ["--cursor-color" as string]: scheme.cursor }}
        >
          {" "}
        </span>
      </div>
    </div>
  );
}

function FontField() {
  const settings = useTerminalSettings();
  const { data: systemFonts = [], isLoading } = useSystemFonts();
  const rescan = useRescanFonts();
  const iconFont = useIconFont();
  const current = findFont(settings.fontId);

  const data = useMemo(() => {
    const installed = (monospace: boolean) =>
      systemFonts
        .filter((font) => font.monospace === monospace)
        .map((font) => ({ value: systemFontId(font.family), label: font.family }));
    const monospace = installed(true);
    const missing =
      current.source === "system" &&
      !systemFonts.some((font) => font.family === current.family) &&
      !isLoading;
    if (missing) monospace.unshift({ value: current.id, label: `${current.family} (not installed)` });
    return [
      { group: "Bundled", items: terminalFonts.map((font) => ({ value: font.id, label: font.label })) },
      { group: "Installed · monospace", items: monospace },
      { group: "Installed · other", items: installed(false) },
    ];
  }, [systemFonts, current.id, current.family, current.source, isLoading]);

  return (
    <Stack gap={4}>
      <Group gap={6} align="flex-end" wrap="nowrap">
        <Select
          label="Font"
          style={{ flex: 1 }}
          searchable
          allowDeselect={false}
          nothingFoundMessage="No font with that name"
          maxDropdownHeight={320}
          limit={1000}
          value={settings.fontId}
          onChange={(value) => value && settings.update({ fontId: value })}
          data={data}
          renderOption={({ option }) => {
            const font = findFont(option.value);
            const preview =
              font.source !== "system" ||
              systemFonts.some((item) => item.monospace && item.family === font.family);
            return <span style={preview ? { fontFamily: fontStack(font) } : undefined}>{option.label}</span>;
          }}
        />
        <Tooltip label="Rescan installed fonts">
          <ActionIcon
            size={36}
            variant="default"
            loading={rescan.isPending}
            onClick={() => rescan.mutate()}
            aria-label="Rescan installed fonts"
          >
            <IconRefresh size={16} />
          </ActionIcon>
        </Tooltip>
      </Group>
      <Text size="xs" c="dimmed">
        {iconFont
          ? `Icons and Powerline symbols use ${iconFont}.`
          : "Install a Nerd Font (for example Symbols Nerd Font) to show prompt icons."}
      </Text>
    </Stack>
  );
}

export function TerminalSettingsSection() {
  const settings = useTerminalSettings();
  const update = settings.update;
  const iconFont = useIconFont();

  return (
    <SettingsSection title="Terminal" description="Color scheme, font and cursor for every terminal tab.">
      <Preview iconFont={iconFont} />
      <Stack gap={8}>
        <Text fw={500} size="sm" c="dimmed">
          Color scheme
        </Text>
        <SimpleGrid cols={{ base: 2, sm: 3, md: 4 }} spacing="sm">
          {terminalSchemes.map((scheme) => (
            <SchemeSwatch
              key={scheme.id}
              scheme={scheme}
              selected={settings.schemeId === scheme.id}
              onSelect={() => update({ schemeId: scheme.id })}
            />
          ))}
        </SimpleGrid>
      </Stack>
      <SimpleGrid cols={{ base: 1, sm: 2 }} spacing="md">
        <FontField />
        <Stack gap={4}>
          <Text fw={500} size="sm" c="dimmed">
            Font size · {settings.fontSize}px
          </Text>
          <Slider
            min={10}
            max={24}
            step={1}
            value={settings.fontSize}
            onChange={(value) => update({ fontSize: value })}
            marks={[{ value: 12 }, { value: 14 }, { value: 16 }, { value: 18 }, { value: 20 }]}
            label={null}
            mt={10}
          />
        </Stack>
        <NumberInput
          label="Line height"
          min={1}
          max={2}
          step={0.05}
          decimalScale={2}
          value={settings.lineHeight}
          onChange={(value) => typeof value === "number" && update({ lineHeight: value })}
        />
        <NumberInput
          label="Letter spacing"
          min={-2}
          max={4}
          step={0.5}
          decimalScale={1}
          value={settings.letterSpacing}
          onChange={(value) => typeof value === "number" && update({ letterSpacing: value })}
        />
        <Stack gap={4}>
          <Text fw={500} size="sm" c="dimmed">
            Cursor
          </Text>
          <SegmentedControl
            value={settings.cursorStyle}
            onChange={(value) =>
              update({ cursorStyle: cursorStyles.find((item) => item.value === value)?.value ?? "block" })
            }
            data={cursorStyles}
          />
        </Stack>
        <NumberInput
          label="Scrollback lines"
          min={500}
          max={100000}
          step={1000}
          thousandSeparator=","
          value={settings.scrollback}
          onChange={(value) => typeof value === "number" && update({ scrollback: value })}
        />
      </SimpleGrid>
      <SimpleGrid cols={{ base: 1, sm: 2 }} spacing="sm">
        <Switch
          label="Blinking cursor"
          checked={settings.cursorBlink}
          onChange={(event) => update({ cursorBlink: event.currentTarget.checked })}
        />
        <Switch
          label="Bright colors for bold text"
          checked={settings.brightBold}
          onChange={(event) => update({ brightBold: event.currentTarget.checked })}
        />
        <Switch
          label="Use Option as Meta key"
          checked={settings.optionAsMeta}
          onChange={(event) => update({ optionAsMeta: event.currentTarget.checked })}
        />
        <Switch
          label="Copy on select"
          checked={settings.copyOnSelect}
          onChange={(event) => update({ copyOnSelect: event.currentTarget.checked })}
        />
        <Switch
          label="Right-click to paste"
          checked={settings.rightClickPaste}
          onChange={(event) => update({ rightClickPaste: event.currentTarget.checked })}
        />
      </SimpleGrid>
      <Group>
        <Button variant="default" leftSection={<IconRestore size={16} />} onClick={settings.reset}>
          Reset terminal settings
        </Button>
      </Group>
    </SettingsSection>
  );
}
