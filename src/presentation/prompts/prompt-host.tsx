import { Alert, Button, Code, Group, Modal, PasswordInput, Stack, Text, TextInput } from "@mantine/core";
import { IconAlertTriangle } from "@tabler/icons-react";
import { useEffect, useState } from "react";
import { notifyError } from "@/application/errors";
import { usePrompts } from "@/application/stores/prompts";
import type { PromptRequest, PromptResponse } from "@/domain/types";
import { api } from "@/infrastructure/tauri/api";

function Actions({
  onCancel,
  submitLabel,
  danger,
}: {
  onCancel: () => void;
  submitLabel: string;
  danger?: boolean;
}) {
  return (
    <Group justify="flex-end" mt="sm">
      <Button variant="default" onClick={onCancel}>
        Cancel
      </Button>
      <Button type="submit" color={danger ? "red" : undefined}>
        {submitLabel}
      </Button>
    </Group>
  );
}

function PromptBody({
  request,
  respond,
}: {
  request: PromptRequest;
  respond: (response: PromptResponse) => void;
}) {
  const kind = request.kind;
  const fieldCount = kind.type === "keyboardInteractive" ? kind.prompts.length : 1;
  const [answers, setAnswers] = useState<string[]>(() => Array(fieldCount).fill(""));
  const cancel = () => respond({ type: "cancel" });

  useEffect(() => {
    setAnswers(Array(fieldCount).fill(""));
  }, [request.id, fieldCount]);

  const setAnswer = (index: number, value: string) =>
    setAnswers((current) => current.map((item, position) => (position === index ? value : item)));

  if (kind.type === "hostKey") {
    const changed = kind.previousFingerprint !== null;
    return (
      <form
        onSubmit={(event) => {
          event.preventDefault();
          respond({ type: "accept" });
        }}
      >
        <Stack gap="sm">
          {changed ? (
            <Alert color="red" icon={<IconAlertTriangle size={18} />} title="Host key changed">
              The key for this server is different from the one you trusted before. Someone may be
              intercepting the connection. Only continue if you know the server key was changed.
            </Alert>
          ) : (
            <Text size="sm">
              This is the first connection to this server. Check the fingerprint before you trust it.
            </Text>
          )}
          <Text size="sm">
            <b>Host:</b> {kind.host}:{kind.port}
          </Text>
          <Text size="sm">
            <b>Key type:</b> {kind.keyType}
          </Text>
          <div>
            <Text size="sm" fw={700}>
              Fingerprint
            </Text>
            <Code block>{kind.fingerprint}</Code>
          </div>
          {changed && (
            <div>
              <Text size="sm" fw={700}>
                Previous fingerprint
              </Text>
              <Code block>{kind.previousFingerprint}</Code>
            </div>
          )}
          <Actions onCancel={cancel} submitLabel={changed ? "Trust new key" : "Trust and connect"} danger={changed} />
        </Stack>
      </form>
    );
  }

  return (
    <form
      onSubmit={(event) => {
        event.preventDefault();
        respond({ type: "answers", answers });
      }}
    >
      <Stack gap="sm">
        {kind.type === "password" && (
          <>
            <Text size="sm" c="dimmed">
              {kind.username}@{kind.host}
            </Text>
            <PasswordInput
              label="Password"
              autoFocus
              data-autofocus
              value={answers[0] ?? ""}
              onChange={(event) => setAnswer(0, event.currentTarget.value)}
            />
          </>
        )}
        {kind.type === "passphrase" && (
          <PasswordInput
            label={`Passphrase for ${kind.keyLabel}`}
            autoFocus
            data-autofocus
            value={answers[0] ?? ""}
            onChange={(event) => setAnswer(0, event.currentTarget.value)}
          />
        )}
        {kind.type === "keyboardInteractive" && (
          <>
            {kind.name && <Text fw={600}>{kind.name}</Text>}
            {kind.instructions && (
              <Text size="sm" c="dimmed" style={{ whiteSpace: "pre-wrap" }}>
                {kind.instructions}
              </Text>
            )}
            {kind.prompts.map((prompt, index) =>
              prompt.echo ? (
                <TextInput
                  key={index}
                  label={prompt.text}
                  autoFocus={index === 0}
                  value={answers[index] ?? ""}
                  onChange={(event) => setAnswer(index, event.currentTarget.value)}
                />
              ) : (
                <PasswordInput
                  key={index}
                  label={prompt.text}
                  autoFocus={index === 0}
                  value={answers[index] ?? ""}
                  onChange={(event) => setAnswer(index, event.currentTarget.value)}
                />
              ),
            )}
          </>
        )}
        <Actions onCancel={cancel} submitLabel="Continue" />
      </Stack>
    </form>
  );
}

export function PromptHost() {
  const head = usePrompts((state) => state.queue[0]);
  const shift = usePrompts((state) => state.shift);

  const respond = (response: PromptResponse) => {
    if (!head) return;
    shift();
    api.prompts.respond(head.id, response).catch(notifyError);
  };

  return (
    <Modal
      opened={Boolean(head)}
      onClose={() => respond({ type: "cancel" })}
      title={<Text fw={700}>{head?.title ?? ""}</Text>}
      closeOnClickOutside={false}
      centered
    >
      {head && <PromptBody key={head.id} request={head} respond={respond} />}
    </Modal>
  );
}
