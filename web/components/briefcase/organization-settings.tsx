'use client';
import { useRef, useState, type SubmitEvent } from 'react';
import { Settings, RefreshCw, CheckCircle2 } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
} from '@/components/ui/dialog';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { api, ApiError, bytes, type BrowserSession } from '@/lib/api';

type UsageMeasure = {
  used_bytes: number;
  remaining_bytes: number;
  limit_bytes: number;
};
type Usage = {
  storage: UsageMeasure;
  daily_uploads: UsageMeasure & { resets_at: string };
};
type Configuration = {
  bucket_name: string;
  region: string;
  role_arn: string;
  prefix: string;
  aws_account_id: string;
  encryption_mode: 'sse_s3' | 'sse_kms';
  kms_key_arn: string | null;
};
type Probe = {
  status: 'configured' | 'failed';
  tested_at: string;
  failure_reason: string | null;
};
const blank: Configuration = {
  bucket_name: '',
  region: '',
  role_arn: '',
  prefix: 'briefcase',
  aws_account_id: '',
  encryption_mode: 'sse_s3',
  kms_key_arn: null,
};

export default function OrganizationSettings({
  session,
  onUnauthorized,
}: {
  session: BrowserSession;
  onUnauthorized: () => void;
}) {
  const [open, setOpen] = useState(false),
    [tab, setTab] = useState('usage');
  const [usage, setUsage] = useState<Usage | null>(null),
    [loading, setLoading] = useState(false),
    [usageError, setUsageError] = useState('');
  const [configuration, setConfiguration] = useState<Configuration>(blank);
  const [result, setResult] = useState<Probe | null>(null),
    [error, setError] = useState(''),
    [busy, setBusy] = useState(false);
  const [uncertain, setUncertain] = useState(false),
    [operation, setOperation] = useState<string | null>(null);
  const intent = useRef<{
    configuration: Configuration;
    operation_id: string;
  } | null>(null);

  async function refreshUsage() {
    setLoading(true);
    setUsageError('');
    try {
      setUsage(await api<Usage>('/usage'));
    } catch (error) {
      if (error instanceof ApiError && error.status === 401) onUnauthorized();
      else
        setUsageError(
          error instanceof Error ? error.message : 'Usage could not be loaded.',
        );
    } finally {
      setLoading(false);
    }
  }
  function change<K extends keyof Configuration>(
    key: K,
    value: Configuration[K],
  ) {
    setConfiguration((previous) => ({
      ...previous,
      [key]: value,
      ...(key === 'encryption_mode' && value === 'sse_s3'
        ? { kms_key_arn: null }
        : {}),
    }));
    setResult(null);
    setError('');
    intent.current = null;
    setOperation(null);
  }
  async function save(event: SubmitEvent) {
    event.preventDefault();
    if (!intent.current)
      intent.current = {
        configuration: { ...configuration },
        operation_id: crypto.randomUUID(),
      };
    setOperation(intent.current.operation_id);
    setBusy(true);
    setError('');
    setResult(null);
    try {
      const probe = await api<Probe>(
        '/storage/configuration',
        'PUT',
        intent.current,
      );
      setResult(probe);
      setUncertain(false);
      intent.current = null;
      if (probe.status === 'configured') await refreshUsage();
    } catch (error) {
      // A transport or intermediary failure cannot prove whether the durable
      // activation happened. Keep the exact intent and lock fields until it is
      // recovered, rather than silently starting a different probe.
      const rejected =
        error instanceof ApiError &&
        [400, 403, 404, 405, 413, 415, 422].includes(error.status);
      setUncertain(!rejected);
      if (rejected) intent.current = null;
      if (error instanceof ApiError && error.status === 401) onUnauthorized();
      else
        setError(
          error instanceof Error
            ? error.message
            : 'The configuration response was lost.',
        );
    } finally {
      setBusy(false);
    }
  }
  const locked = busy || uncertain;
  return (
    <>
      <Button
        variant="ghost"
        className="organization-settings-trigger"
        onClick={() => {
          setOpen(true);
          void refreshUsage();
        }}
      >
        <Settings size={17} /> Organisation settings
      </Button>
      <Dialog
        open={open}
        onOpenChange={(value) => {
          if (!busy) setOpen(value);
        }}
      >
        <DialogContent className="organization-settings-dialog sm:max-w-2xl">
          <DialogHeader>
            <DialogTitle>Organisation settings</DialogTitle>
            <DialogDescription>
              {session.org}
              {session.testing ? ' · Testing environment' : ''}
            </DialogDescription>
          </DialogHeader>
          <Tabs value={tab} onValueChange={setTab}>
            <TabsList>
              <TabsTrigger value="usage">Usage</TabsTrigger>
              <TabsTrigger value="storage">Storage configuration</TabsTrigger>
            </TabsList>
            <TabsContent value="usage">
              <div className="settings-section-heading">
                <h3>Storage and uploads</h3>
                <Button
                  variant="ghost"
                  disabled={loading}
                  onClick={() => void refreshUsage()}
                >
                  <RefreshCw size={15} /> Refresh
                </Button>
              </div>
              {loading && <output>Loading usage…</output>}
              {usageError && (
                <p className="error-box" role="alert">
                  {usageError}
                </p>
              )}
              {usage && (
                <>
                  <dl className="usage-details">
                    {[
                      { title: 'Stored data', data: usage.storage },
                      { title: 'Uploads today', data: usage.daily_uploads },
                    ].map(({ title, data }) => (
                      <div key={title}>
                        <dt>{title}</dt>
                        <dd>
                          <strong>{bytes(data.used_bytes)}</strong>
                          <span>
                            {data.used_bytes.toLocaleString()} bytes used
                          </span>
                          <span>
                            {bytes(data.remaining_bytes)} remaining of{' '}
                            {bytes(data.limit_bytes)}
                          </span>
                        </dd>
                      </div>
                    ))}
                  </dl>
                  <p className="detail-hint">
                    Upload allowance resets at{' '}
                    {new Date(usage.daily_uploads.resets_at).toLocaleString()}{' '}
                    (midnight UTC). Stored data includes retained versions and
                    items in the bin.
                  </p>
                </>
              )}
            </TabsContent>
            <TabsContent value="storage">
              <h3 className="settings-section-heading">
                Use your own S3 bucket
              </h3>
              <p className="detail-hint">
                An organisation owner or authorised administrator can configure
                storage. Briefcase assumes your AWS role; do not enter AWS
                access keys or IAM application secrets.
              </p>
              <form onSubmit={save}>
                <fieldset disabled={locked} className="storage-fields">
                  <legend className="sr-only">S3 configuration</legend>
                  <label htmlFor="storage-bucket">
                    Bucket name
                    <Input
                      id="storage-bucket"
                      required
                      minLength={3}
                      maxLength={63}
                      value={configuration.bucket_name}
                      onChange={(event) =>
                        change('bucket_name', event.target.value)
                      }
                      autoComplete="off"
                      placeholder="organisation-files"
                    />
                  </label>
                  <label htmlFor="storage-region">
                    AWS region
                    <Input
                      id="storage-region"
                      required
                      maxLength={64}
                      value={configuration.region}
                      onChange={(event) => change('region', event.target.value)}
                      autoComplete="off"
                      placeholder="ap-south-1"
                    />
                  </label>
                  <label htmlFor="storage-account">
                    AWS account ID
                    <Input
                      id="storage-account"
                      required
                      pattern="[0-9]{12}"
                      inputMode="numeric"
                      maxLength={12}
                      value={configuration.aws_account_id}
                      onChange={(event) =>
                        change('aws_account_id', event.target.value)
                      }
                      autoComplete="off"
                      placeholder="12-digit account ID"
                    />
                  </label>
                  <label htmlFor="storage-prefix">
                    Bucket prefix
                    <Input
                      id="storage-prefix"
                      required
                      maxLength={512}
                      value={configuration.prefix}
                      onChange={(event) => change('prefix', event.target.value)}
                      autoComplete="off"
                    />
                  </label>
                  <label className="full-width" htmlFor="storage-role">
                    IAM role ARN
                    <Input
                      id="storage-role"
                      required
                      maxLength={2048}
                      value={configuration.role_arn}
                      onChange={(event) =>
                        change('role_arn', event.target.value)
                      }
                      autoComplete="off"
                      placeholder="arn:aws:iam::123456789012:role/briefcase"
                    />
                  </label>
                  <div className="full-width">
                    <label
                      id="storage-encryption-label"
                      htmlFor="storage-encryption"
                    >
                      Encryption
                    </label>
                    <Select
                      value={configuration.encryption_mode}
                      disabled={locked}
                      onValueChange={(value) => {
                        if (value === 'sse_s3' || value === 'sse_kms')
                          change('encryption_mode', value);
                      }}
                    >
                      <SelectTrigger
                        id="storage-encryption"
                        aria-labelledby="storage-encryption-label"
                      >
                        <SelectValue>
                          {configuration.encryption_mode === 'sse_s3'
                            ? 'S3-managed keys (SSE-S3)'
                            : 'KMS key (SSE-KMS)'}
                        </SelectValue>
                      </SelectTrigger>
                      <SelectContent>
                        <SelectItem value="sse_s3">
                          S3-managed keys (SSE-S3)
                        </SelectItem>
                        <SelectItem value="sse_kms">
                          KMS key (SSE-KMS)
                        </SelectItem>
                      </SelectContent>
                    </Select>
                  </div>
                  {configuration.encryption_mode === 'sse_kms' && (
                    <label className="full-width" htmlFor="storage-kms">
                      KMS key ARN
                      <Input
                        id="storage-kms"
                        required
                        maxLength={2048}
                        value={configuration.kms_key_arn || ''}
                        onChange={(event) =>
                          change('kms_key_arn', event.target.value)
                        }
                        autoComplete="off"
                      />
                      <span className="detail-hint">
                        Use a key in the bucket’s account and region.
                      </span>
                    </label>
                  )}
                </fieldset>
                <p className="detail-hint">
                  Saving writes, reads, updates, and deletes a temporary probe
                  object. The bucket is activated only after every check and
                  cleanup succeeds. New file versions use the activated bucket;
                  existing versions keep their recorded location.
                </p>
                {error && (
                  <p className="error-box" role="alert">
                    {error}
                  </p>
                )}
                {uncertain && (
                  <p className="detail-hint">
                    The result is not confirmed. Retry the same configuration to
                    recover its outcome. Keep this page open; the unchanged
                    request is retained here.
                  </p>
                )}
                {result && (
                  <output
                    className={
                      result.status === 'configured' ? 'notice' : 'error-box'
                    }
                  >
                    {result.status === 'configured' ? (
                      <>
                        <CheckCircle2 size={18} /> S3 bucket configured.
                      </>
                    ) : (
                      <>
                        Validation failed. The previous storage configuration
                        remains selected.{' '}
                        {result.failure_reason
                          ? `Reason: ${result.failure_reason}.`
                          : ''}
                      </>
                    )}
                    <span>
                      Checked {new Date(result.tested_at).toLocaleString()}.
                    </span>
                  </output>
                )}
                {operation && (
                  <p className="settings-operation">
                    Operation ID: <code>{operation}</code>
                  </p>
                )}
                <Button
                  type="submit"
                  disabled={busy || result?.status === 'configured'}
                >
                  {busy
                    ? 'Validating and configuring…'
                    : uncertain
                      ? 'Retry same configuration'
                      : result?.status === 'failed'
                        ? 'Run a new validation'
                        : result?.status === 'configured'
                          ? 'Bucket configured'
                          : 'Validate and configure bucket'}
                </Button>
              </form>
            </TabsContent>
          </Tabs>
        </DialogContent>
      </Dialog>
    </>
  );
}
