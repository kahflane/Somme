"use client";

import dynamic from "next/dynamic";

const AppRoot = dynamic(
  () => import("@/presentation/app-root").then((module) => module.AppRoot),
  { ssr: false },
);

export default function Page() {
  return <AppRoot />;
}
