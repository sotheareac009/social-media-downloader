import { Button } from "@/components/ui/Button";
import { LinkIcon, XIcon } from "@/components/ui/icons";
import type { AccountView, ProviderDescriptor } from "@/lib/auth";

interface Props {
  descriptor: ProviderDescriptor;
  account: AccountView;
  busy: boolean;
  /** True while some *other* provider is mid-flow; only one runs at a time. */
  blocked: boolean;
  onConnect: () => void;
  onDisconnect: () => void;
}

export function ConnectButton({
  descriptor,
  account,
  busy,
  blocked,
  onConnect,
  onDisconnect,
}: Props) {
  if (account.connected) {
    return (
      <Button
        variant="danger"
        loading={busy}
        disabled={blocked}
        icon={<XIcon size={14} />}
        onClick={onDisconnect}
        aria-label={`Disconnect ${descriptor.display_name}`}
      >
        {busy ? "Disconnecting" : "Disconnect"}
      </Button>
    );
  }

  if (!descriptor.configured) {
    return (
      <Button variant="ghost" disabled>
        Unavailable
      </Button>
    );
  }

  return (
    <Button
      variant="primary"
      loading={busy}
      disabled={blocked}
      icon={<LinkIcon size={14} />}
      onClick={onConnect}
      aria-label={`Connect ${descriptor.display_name}`}
    >
      {busy ? "Waiting for browser" : "Connect"}
    </Button>
  );
}
