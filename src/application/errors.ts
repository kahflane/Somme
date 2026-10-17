import { notifications } from "@mantine/notifications";
import type { ApiError } from "@/domain/types";

function isApiError(value: unknown): value is ApiError {
  return (
    typeof value === "object" &&
    value !== null &&
    "message" in value &&
    typeof (value as { message: unknown }).message === "string"
  );
}

export function errorMessage(error: unknown): string {
  if (isApiError(error)) return error.message;
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  return "Something went wrong";
}

export function notifyError(error: unknown, title = "Error") {
  notifications.show({ color: "red", title, message: errorMessage(error) });
}

export function notifySuccess(message: string, title?: string) {
  notifications.show({ color: "blue", title, message });
}
