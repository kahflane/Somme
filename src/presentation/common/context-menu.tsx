import { Menu } from "@mantine/core";
import { useState, type MouseEvent, type ReactNode } from "react";

interface Position {
  x: number;
  y: number;
}

export function useContextMenu<T>() {
  const [state, setState] = useState<{ target: T; position: Position } | null>(null);
  return {
    target: state?.target ?? null,
    position: state?.position ?? null,
    open: (event: MouseEvent, target: T) => {
      event.preventDefault();
      setState({ target, position: { x: event.clientX, y: event.clientY } });
    },
    close: () => setState(null),
  };
}

interface Props {
  position: Position | null;
  onClose: () => void;
  children: ReactNode;
}

export function ContextMenu({ position, onClose, children }: Props) {
  return (
    <Menu opened={position !== null} onClose={onClose} position="bottom-start" offset={2} withinPortal>
      <Menu.Target>
        <div
          style={{
            position: "fixed",
            left: position?.x ?? 0,
            top: position?.y ?? 0,
            width: 1,
            height: 1,
            pointerEvents: "none",
          }}
        />
      </Menu.Target>
      <Menu.Dropdown onClick={onClose}>{children}</Menu.Dropdown>
    </Menu>
  );
}
