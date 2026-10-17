import type { SVGProps } from "react";

interface GlyphProps extends Omit<SVGProps<SVGSVGElement>, "stroke"> {
  size?: number;
  stroke?: number;
}

function Glyph({ size = 18, stroke: _stroke, children, ...props }: GlyphProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="currentColor" aria-hidden="true" {...props}>
      {children}
    </svg>
  );
}

export function ServerGlyph(props: GlyphProps) {
  return (
    <Glyph {...props}>
      <path
        fillRule="evenodd"
        d="M5 3h14a3 3 0 0 1 3 3v2a3 3 0 0 1-3 3H5a3 3 0 0 1-3-3V6a3 3 0 0 1 3-3Zm1 3v2h1.6V6H6Zm3.2 0v2h1.6V6H9.2Zm3.2 0v2H14V6h-1.6ZM18 5.8a1.2 1.2 0 1 0 0 2.4 1.2 1.2 0 0 0 0-2.4ZM5 13h14a3 3 0 0 1 3 3v2a3 3 0 0 1-3 3H5a3 3 0 0 1-3-3v-2a3 3 0 0 1 3-3Zm1 3v2h1.6v-2H6Zm3.2 0v2h1.6v-2H9.2Zm3.2 0v2H14v-2h-1.6Zm5.6-.2a1.2 1.2 0 1 0 0 2.4 1.2 1.2 0 0 0 0-2.4Z"
      />
    </Glyph>
  );
}

export function KeyGlyph(props: GlyphProps) {
  return (
    <Glyph {...props}>
      <path
        fillRule="evenodd"
        d="M12 1.5a5.5 5.5 0 0 0-1.75 10.71V20.4L12 22.5l1.75-2.1v-1.65l-1.05-1.1 1.05-1.1v-1.1l-1.05-1.1 1.05-1.1v-.04A5.5 5.5 0 0 0 12 1.5Zm0 3.2a1.8 1.8 0 1 1 0 3.6 1.8 1.8 0 0 1 0-3.6Z"
      />
    </Glyph>
  );
}

export function VaultGlyph(props: GlyphProps) {
  return (
    <Glyph {...props}>
      <path
        fillRule="evenodd"
        d="M6 3h12a4 4 0 0 1 4 4v8a4 4 0 0 1-4 4h-.5l-.6 1.6a.6.6 0 0 1-.56.4h-1.68a.6.6 0 0 1-.56-.4L13.5 19h-3l-.6 1.6a.6.6 0 0 1-.56.4H7.66a.6.6 0 0 1-.56-.4L6.5 19H6a4 4 0 0 1-4-4V7a4 4 0 0 1 4-4Zm6 3.5a4.5 4.5 0 1 0 0 9 4.5 4.5 0 0 0 0-9Zm0 2.5a2 2 0 1 1 0 4 2 2 0 0 1 0-4ZM4.5 9.5v3h1.5v-3H4.5Z"
      />
    </Glyph>
  );
}

export function TerminalGlyph(props: GlyphProps) {
  return (
    <Glyph {...props}>
      <path
        fillRule="evenodd"
        d="M5 3h14a3 3 0 0 1 3 3v12a3 3 0 0 1-3 3H5a3 3 0 0 1-3-3V6a3 3 0 0 1 3-3Zm1.3 6.2a.9.9 0 0 0 0 1.3l2.3 2.2-2.3 2.2a.9.9 0 1 0 1.3 1.3l2.95-2.85a.9.9 0 0 0 0-1.3L7.6 9.2a.9.9 0 0 0-1.3 0ZM12 15a.9.9 0 1 0 0 1.8h4.5a.9.9 0 1 0 0-1.8H12Z"
      />
    </Glyph>
  );
}

export function ForwardGlyph(props: GlyphProps) {
  return (
    <Glyph {...props}>
      <path d="M13.2 4.4a1 1 0 0 1 1.62-.78l7.5 6.6a1 1 0 0 1 0 1.5l-7.5 6.6a1 1 0 0 1-1.62-.78V14H9.1l1.7-6h2.4V4.4ZM5.3 8h3l-1.7 6h-3l1.7-6Zm-3.6 0h1.5l-1.7 6H0l1.7-6Z" />
    </Glyph>
  );
}
