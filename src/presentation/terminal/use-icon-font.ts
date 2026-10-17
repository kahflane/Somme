import { useMemo } from "react";
import { useSystemFonts } from "@/application/hooks/system-fonts";
import { pickIconFont } from "@/presentation/terminal/fonts";

export function useIconFont(): string | null {
  const { data: fonts } = useSystemFonts();
  return useMemo(() => pickIconFont((fonts ?? []).map((font) => font.family)), [fonts]);
}
