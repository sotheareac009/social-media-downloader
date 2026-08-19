import { Button } from "@/components/ui/Button";
import { LinkIcon, XIcon } from "@/components/ui/icons";
import type { AccountView, ProviderDescriptor } from "@/lib/auth";

interface Props {
  descriptor: ProviderDescriptor;
  account: AccountView;
  busy: boolean;
  /** True while some *other* provider is mid-flow; only one runs at a time. */
  blocked: boolean;
  /** A working download session exists for this provider. */
  downloadConnected?: boolean;
  /**
   * Starts a *download* sign-in — the same flow the Downloads page runs.
   * Offered for providers that have no OAuth client ID but do support a
   * captured session, so the card has a working action instead of a dead one.
   */
  onDownloadSignIn?: () => void;
  downloadBusy?: boolean;
  onConnect: () => void;
  onDisconnect: () => void;
}

export function ConnectButton({
  descriptor,
  account,
  busy,
  blocked,
  downloadConnected = false,
  onDownloadSignIn,
  downloadBusy = false,
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
    // Already signed in for downloads: account sign-in is optional and OAuth
    // is not configured, so there is genuinely nothing to offer here.
    if (downloadConnected) return null;

    // No OAuth, but a session sign-in is available — give the real action
    // rather than a permanently disabled button.
    if (onDownloadSignIn) {
      return (
        <Button
          variant="primary"
          loading={downloadBusy}
          icon={<LinkIcon size={14} />}
          onClick={onDownloadSignIn}
          aria-label={`Sign in to ${descriptor.display_name}`}
        >
          {downloadBusy ? "Waiting for sign-in" : `Sign in to ${descriptor.display_name}`}
        </Button>
      );
    }

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
