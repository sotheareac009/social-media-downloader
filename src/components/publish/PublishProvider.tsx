import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import {
  ldplayerEnvironment,
  ldplayerListDevices,
  subscribeToDeviceEvents,
  type DeviceEnvironment,
  type DeviceView,
  type LogLine,
} from "@/lib/ldplayer";
import {
  publishAccounts,
  publishJobs,
  subscribeToPublishEvents,
  type AccountView,
  type PublishJob,
} from "@/lib/publish";

/**
 * One place that holds devices, accounts and jobs.
 *
 * WHY A PROVIDER. Four pages read the same three lists, and listing devices is
 * not cheap — it shells out to `ldconsole` and to `adb` once per instance. If
 * each page fetched for itself, switching tabs would spawn a burst of adb
 * calls, and adb under concurrent load is exactly when emulators start
 * reporting `offline`. Fetch once, share, and let events keep it fresh.
 */
interface PublishState {
  environment: DeviceEnvironment | null;
  devices: DeviceView[];
  accounts: AccountView[];
  jobs: PublishJob[];
  logs: LogLine[];
  /** True while a device refresh is in flight — it is slow enough to show. */
  scanning: boolean;
  refreshDevices: () => Promise<void>;
  refreshAccounts: () => Promise<void>;
  refreshJobs: () => Promise<void>;
  refreshEnvironment: () => Promise<void>;
  clearLogs: () => void;
}

const noop = async () => {};

const PublishContext = createContext<PublishState>({
  environment: null,
  devices: [],
  accounts: [],
  jobs: [],
  logs: [],
  scanning: false,
  refreshDevices: noop,
  refreshAccounts: noop,
  refreshJobs: noop,
  refreshEnvironment: noop,
  clearLogs: () => {},
});

export const usePublish = () => useContext(PublishContext);

/** Keep the log pane bounded; a verbose publish emits a line per adb call. */
const MAX_LOGS = 400;

export function PublishProvider({ children }: { children: ReactNode }) {
  const [environment, setEnvironment] = useState<DeviceEnvironment | null>(null);
  const [devices, setDevices] = useState<DeviceView[]>([]);
  const [accounts, setAccounts] = useState<AccountView[]>([]);
  const [jobs, setJobs] = useState<PublishJob[]>([]);
  const [logs, setLogs] = useState<LogLine[]>([]);
  const [scanning, setScanning] = useState(false);

  // A device scan takes seconds. Overlapping scans produce contradictory
  // results and hammer adb, so a second request while one is running is
  // dropped rather than queued.
  const scanning_ = useRef(false);

  const refreshEnvironment = useCallback(async () => {
    try {
      setEnvironment(await ldplayerEnvironment());
    } catch {
      setEnvironment(null);
    }
  }, []);

  const refreshDevices = useCallback(async () => {
    if (scanning_.current) return;
    scanning_.current = true;
    setScanning(true);
    try {
      setDevices(await ldplayerListDevices());
    } catch {
      // A missing adb is reported by the environment banner, not by an empty
      // list pretending to be an answer.
      setDevices([]);
    } finally {
      scanning_.current = false;
      setScanning(false);
    }
  }, []);

  const refreshAccounts = useCallback(async () => {
    try {
      setAccounts(await publishAccounts());
    } catch {
      setAccounts([]);
    }
  }, []);

  const refreshJobs = useCallback(async () => {
    try {
      setJobs(await publishJobs());
    } catch {
      setJobs([]);
    }
  }, []);

  useEffect(() => {
    void refreshEnvironment();
    void refreshDevices();
    void refreshAccounts();
    void refreshJobs();
  }, [refreshEnvironment, refreshDevices, refreshAccounts, refreshJobs]);

  useEffect(() => {
    const pending = subscribeToDeviceEvents({
      onDevices: setDevices,
      onDevice: (device) =>
        setDevices((prev) => prev.map((d) => (d.id === device.id ? device : d))),
      onLog: (line) => setLogs((prev) => [...prev, line].slice(-MAX_LOGS)),
    });
    return () => {
      void pending.then((un) => un());
    };
  }, []);

  useEffect(() => {
    const upsert = (job: PublishJob) =>
      setJobs((prev) => {
        const at = prev.findIndex((j) => j.id === job.id);
        if (at === -1) return [job, ...prev];
        const next = [...prev];
        next[at] = job;
        return next;
      });

    const pending = subscribeToPublishEvents({
      onCreated: upsert,
      onUpdated: upsert,
      // A finished job may have been the reason a device was busy, and its
      // account's status can have changed (app crashed, emulator stopped).
      onFinished: (job) => {
        upsert(job);
        void refreshAccounts();
      },
    });
    return () => {
      void pending.then((un) => un());
    };
  }, [refreshAccounts]);

  const value = useMemo<PublishState>(
    () => ({
      environment,
      devices,
      accounts,
      jobs,
      logs,
      scanning,
      refreshDevices,
      refreshAccounts,
      refreshJobs,
      refreshEnvironment,
      clearLogs: () => setLogs([]),
    }),
    [
      environment,
      devices,
      accounts,
      jobs,
      logs,
      scanning,
      refreshDevices,
      refreshAccounts,
      refreshJobs,
      refreshEnvironment,
    ],
  );

  return <PublishContext.Provider value={value}>{children}</PublishContext.Provider>;
}
