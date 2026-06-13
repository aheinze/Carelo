<script setup>
import { computed, nextTick, onUnmounted, reactive, ref, watch } from 'vue';
import AppIcon from './AppIcon.vue';
import { addRemoteVolume, createOAuthTokens } from '../composables/useFileOperations';
import { useScrollableContentState } from '../composables/useScrollableContentState';
import { useFileManagerStore } from '../stores/fileManagerStore';

const props = defineProps({
  visible: { type: Boolean, default: false },
});

const emit = defineEmits(['close']);
const store = useFileManagerStore();

const nameInput = ref(null);
const primaryFieldInput = ref(null);
const remoteContent = ref(null);
const saving = ref(false);
const creatingTokens = ref(false);
const errorMessage = ref('');
const oauthMessage = ref('');
const OAUTH_CALLBACK_URL = 'http://127.0.0.1:53682/oauth/callback';
const NETWORK_LOCATION_SCHEME = 'network';
const oauthClientSecretRequired = new Set(['gdrive', 'dropbox']);
const hiddenProviderSchemes = new Set(['b2', 'gdrive', 'onedrive', 'dropbox', 'swift']);
const basicProviderSchemes = new Set([NETWORK_LOCATION_SCHEME, 's3']);
const showAdvancedProviders = ref(false);
const networkProtocolOptions = [
  { value: 'sftp', label: 'SFTP' },
  { value: 'smb', label: 'SMB' },
  { value: 'ftp', label: 'FTP' },
  { value: 'ftps', label: 'FTPS' },
  { value: 'davs', label: 'WebDAV' },
  { value: 'dav', label: 'WebDAV HTTP' },
];
const networkProtocolKinds = {
  ssh: 'sftp',
  sftp: 'sftp',
  smb: 'smb',
  cifs: 'smb',
  ftp: 'ftp',
  ftps: 'ftp',
  dav: 'webdav',
  davs: 'webdav',
  webdav: 'webdav',
  webdavs: 'webdav',
  http: 'webdav',
  https: 'webdav',
};

// ── Protocol custom select ─────────────────────────────────
const protocolDropdownOpen = ref(false);
const protocolTriggerRef = ref(null);
const protocolDropdownRef = ref(null);
const dropdownRect = reactive({ top: 0, left: 0, width: 0 });

function openProtocolDropdown() {
  const el = protocolTriggerRef.value;
  if (el) {
    const r = el.getBoundingClientRect();
    dropdownRect.top = r.bottom + 5;
    dropdownRect.left = r.left;
    dropdownRect.width = r.width;
  }
  protocolDropdownOpen.value = true;
  nextTick(() => {
    document.addEventListener('pointerdown', handleProtocolOutsideClick, { capture: true });
  });
}

function closeProtocolDropdown() {
  protocolDropdownOpen.value = false;
  document.removeEventListener('pointerdown', handleProtocolOutsideClick, { capture: true });
}

function toggleProtocolDropdown() {
  protocolDropdownOpen.value ? closeProtocolDropdown() : openProtocolDropdown();
}

function selectProtocol(scheme) {
  if (!basicProviderSchemes.has(scheme)) {
    showAdvancedProviders.value = true;
  }
  form.scheme = scheme;
  closeProtocolDropdown();
}

function toggleAdvancedProviders() {
  showAdvancedProviders.value = !showAdvancedProviders.value;

  if (!showAdvancedProviders.value && !basicProviderSchemes.has(form.scheme)) {
    form.scheme = NETWORK_LOCATION_SCHEME;
  }
}

function handleProtocolOutsideClick(e) {
  if (
    !protocolTriggerRef.value?.contains(e.target) &&
    !protocolDropdownRef.value?.contains(e.target)
  ) {
    closeProtocolDropdown();
  }
}

onUnmounted(closeProtocolDropdown);

// Field types: 'text' | 'password' | 'select'
// Special: { divider: 'Label' } renders a section heading inside the creds grid.
const providers = [
  {
    label: 'Server Address',
    scheme: NETWORK_LOCATION_SCHEME,
    icon: 'network',
    description: 'SMB, SFTP, FTP, WebDAV',
    rootPlaceholder: '',
    fields: [
      { key: 'protocol', label: 'Protocol', type: 'select', default: 'sftp' },
      { key: 'host', label: 'Host', type: 'text', placeholder: 'server.example.com/path' },
      { key: 'username', label: 'Username', type: 'text', placeholder: '', optional: true, half: true },
      { key: 'password', label: 'Password', type: 'password', placeholder: '', optional: true, half: true },
      { key: 'domain', label: 'Domain / Workgroup', type: 'text', placeholder: 'WORKGROUP', optional: true, half: true },
      { key: 'key', label: 'SSH Private Key / Path', type: 'text', placeholder: '~/.ssh/id_rsa', optional: true },
      { key: 'known_hosts_strategy', label: 'SSH Known Hosts', type: 'select', optional: true, half: true,
        options: [{ value: '', label: '— default (strict) —' }, { value: 'strict', label: 'Strict' }, { value: 'accept', label: 'Accept all' }, { value: 'add', label: 'Add & trust' }] },
    ],
  },
  {
    label: 'SMB / CIFS',
    scheme: 'smb',
    icon: 'server',
    description: 'Windows and NAS shares',
    rootPlaceholder: 'Projects',
    fields: [
      { key: 'endpoint', label: 'Share URL', type: 'text', placeholder: 'smb://server/share' },
      { key: 'username', label: 'Username', type: 'text', placeholder: '', optional: true, half: true },
      { key: 'password', label: 'Password', type: 'password', placeholder: '', optional: true, half: true },
      { key: 'domain', label: 'Domain / Workgroup', type: 'text', placeholder: 'WORKGROUP', optional: true, half: true },
    ],
  },
  {
    label: 'SFTP',
    scheme: 'sftp',
    icon: 'terminal',
    description: 'SSH file transfer',
    rootPlaceholder: 'home/deploy',
    fields: [
      { key: 'endpoint', label: 'Host', type: 'text', placeholder: 'example.com', half: true },
      { key: '_port', label: 'Port', type: 'text', placeholder: '22', optional: true, half: true },
      { key: 'user', label: 'Username', type: 'text', placeholder: 'deploy', optional: true, half: true },
      { key: 'password', label: 'Password', type: 'password', placeholder: '', optional: true, half: true },
      { key: 'key', label: 'Private Key / Path', type: 'text', placeholder: '~/.ssh/id_rsa  or  raw PEM/OpenSSH key', optional: true },
      { key: 'known_hosts_strategy', label: 'Known Hosts Strategy', type: 'select', optional: true, half: true,
        options: [{ value: '', label: '— default (strict) —' }, { value: 'strict', label: 'Strict' }, { value: 'accept', label: 'Accept all' }, { value: 'add', label: 'Add & trust' }] },
      { key: 'enable_copy', label: 'Enable Server-Side Copy', type: 'select', optional: true, half: true,
        options: [{ value: '', label: '— default (false) —' }, { value: 'true', label: 'true' }, { value: 'false', label: 'false' }] },
    ],
  },
  {
    label: 'FTP / FTPS',
    scheme: 'ftp',
    icon: 'server',
    description: 'FTP with optional TLS',
    rootPlaceholder: 'public_html',
    fields: [
      { key: 'endpoint', label: 'Server', type: 'text', placeholder: 'ftp://example.com  or  ftps://example.com', half: true },
      { key: '_port', label: 'Port', type: 'text', placeholder: '21', optional: true, half: true },
      { key: 'user', label: 'Username', type: 'text', placeholder: 'anonymous', optional: true, half: true },
      { key: 'password', label: 'Password', type: 'password', placeholder: '', optional: true, half: true },
    ],
  },
  {
    label: 'WebDAV',
    scheme: 'webdav',
    icon: 'network',
    description: 'HTTP / HTTPS WebDAV',
    rootPlaceholder: 'remote.php/dav/files/user',
    fields: [
      { key: 'endpoint', label: 'Server URL', type: 'text', placeholder: 'https://cloud.example.com' },
      { key: 'username', label: 'Username', type: 'text', placeholder: '', optional: true, half: true },
      { key: 'password', label: 'Password', type: 'password', placeholder: '', optional: true, half: true },
      { key: 'token', label: 'Bearer Token', type: 'password', placeholder: 'OAuth bearer token (replaces user + password)', optional: true },
      { divider: 'Advanced' },
      { key: 'disable_copy', label: 'Disable Copy', type: 'select', optional: true, half: true,
        options: [{ value: '', label: '— default (false) —' }, { value: 'true', label: 'true' }, { value: 'false', label: 'false' }] },
      { key: 'disable_create_dir', label: 'Disable Create Dir', type: 'select', optional: true, half: true,
        options: [{ value: '', label: '— default (false) —' }, { value: 'true', label: 'true' }, { value: 'false', label: 'false' }] },
      { key: 'enable_user_metadata', label: 'User Metadata', type: 'select', optional: true, half: true,
        options: [{ value: '', label: '— default (false) —' }, { value: 'true', label: 'true' }, { value: 'false', label: 'false' }] },
      { key: 'user_metadata_prefix', label: 'Metadata Prefix', type: 'text', placeholder: 'X-OC-', optional: true, half: true },
      { key: 'user_metadata_uri', label: 'Metadata URI', type: 'text', placeholder: 'https://opendal.apache.org/ns', optional: true },
    ],
  },
  {
    label: 'Amazon S3',
    scheme: 's3',
    icon: 'database',
    description: 'AWS S3 or S3-compatible',
    rootPlaceholder: 'projects/archive',
    fields: [
      { key: 'bucket', label: 'Bucket', type: 'text', placeholder: 'my-bucket', half: true },
      { key: 'region', label: 'Region', type: 'text', placeholder: 'us-east-1', half: true },
      { key: 'access_key_id', label: 'Access Key ID', type: 'text', placeholder: '', half: true },
      { key: 'secret_access_key', label: 'Secret Access Key', type: 'password', placeholder: '', half: true },
      { key: 'endpoint', label: 'Custom Endpoint', type: 'text', placeholder: 'https://s3.example.com  (MinIO, R2, etc.)', optional: true },
      { key: 'session_token', label: 'Session Token', type: 'password', placeholder: 'Temporary STS token', optional: true },
      { divider: 'Credentials' },
      { key: 'disable_config_load', label: 'Disable Config Load', type: 'select', optional: true, half: true,
        options: [{ value: '', label: '— default (false) —' }, { value: 'true', label: 'true' }, { value: 'false', label: 'false' }] },
      { key: 'disable_ec2_metadata', label: 'Disable EC2 Metadata', type: 'select', optional: true, half: true,
        options: [{ value: '', label: '— default (false) —' }, { value: 'true', label: 'true' }, { value: 'false', label: 'false' }] },
      { key: 'allow_anonymous', label: 'Allow Anonymous', type: 'select', optional: true, half: true,
        options: [{ value: '', label: '— default (false) —' }, { value: 'true', label: 'true' }, { value: 'false', label: 'false' }] },
      { divider: 'IAM Role' },
      { key: 'role_arn', label: 'Role ARN', type: 'text', placeholder: 'arn:aws:iam::123456789012:role/MyRole', optional: true },
      { key: 'external_id', label: 'External ID', type: 'text', placeholder: '', optional: true, half: true },
      { key: 'role_session_name', label: 'Session Name', type: 'text', placeholder: 'carelo-session', optional: true, half: true },
      { divider: 'Encryption & Storage' },
      { key: 'server_side_encryption', label: 'Server-Side Encryption', type: 'select', optional: true, half: true,
        options: [{ value: '', label: '— none —' }, { value: 'aws:kms', label: 'aws:kms' }, { value: 'AES256', label: 'AES256' }] },
      { key: 'server_side_encryption_aws_kms_key_id', label: 'KMS Key ID', type: 'text', placeholder: '', optional: true, half: true },
      { key: 'server_side_encryption_customer_algorithm', label: 'Customer Key Algorithm', type: 'select', optional: true, half: true,
        options: [{ value: '', label: '— none —' }, { value: 'AES256', label: 'AES256' }] },
      { key: 'server_side_encryption_customer_key', label: 'Customer Key', type: 'password', placeholder: 'Base64 encoded AES-256 key', optional: true },
      { key: 'server_side_encryption_customer_key_md5', label: 'Customer Key MD5', type: 'password', placeholder: 'Base64 encoded MD5 digest', optional: true },
      { key: 'default_storage_class', label: 'Storage Class', type: 'select', optional: true, half: true,
        options: [
          { value: '', label: '— default —' },
          { value: 'STANDARD', label: 'Standard' },
          { value: 'INTELLIGENT_TIERING', label: 'Intelligent-Tiering' },
          { value: 'STANDARD_IA', label: 'Standard-IA' },
          { value: 'ONEZONE_IA', label: 'One Zone-IA' },
          { value: 'EXPRESS_ONEZONE', label: 'Express One Zone' },
          { value: 'GLACIER', label: 'Glacier Flexible Retrieval' },
          { value: 'GLACIER_IR', label: 'Glacier Instant Retrieval' },
          { value: 'DEEP_ARCHIVE', label: 'Deep Archive' },
          { value: 'OUTPOSTS', label: 'Outposts' },
          { value: 'REDUCED_REDUNDANCY', label: 'Reduced Redundancy' },
        ] },
      { key: 'checksum_algorithm', label: 'Checksum Algorithm', type: 'select', optional: true, half: true,
        options: [{ value: '', label: '— none —' }, { value: 'crc32c', label: 'CRC32C' }, { value: 'md5', label: 'MD5' }] },
      { key: 'default_acl', label: 'Default ACL', type: 'text', placeholder: 'private, public-read, bucket-owner-full-control', optional: true },
      { divider: 'Flags' },
      { key: 'enable_virtual_host_style', label: 'Virtual Host Style', type: 'select', optional: true, half: true,
        options: [{ value: '', label: '— default (false) —' }, { value: 'true', label: 'true' }, { value: 'false', label: 'false' }] },
      { key: 'enable_versioning', label: 'Versioning', type: 'select', optional: true, half: true,
        options: [{ value: '', label: '— default (false) —' }, { value: 'true', label: 'true' }, { value: 'false', label: 'false' }] },
      { key: 'disable_list_objects_v2', label: 'Disable List Objects V2', type: 'select', optional: true, half: true,
        options: [{ value: '', label: '— default (false) —' }, { value: 'true', label: 'true' }, { value: 'false', label: 'false' }] },
      { key: 'enable_request_payer', label: 'Request Payer', type: 'select', optional: true, half: true,
        options: [{ value: '', label: '— default (false) —' }, { value: 'true', label: 'true' }, { value: 'false', label: 'false' }] },
      { key: 'disable_stat_with_override', label: 'Disable Stat Override', type: 'select', optional: true, half: true,
        options: [{ value: '', label: '— default (false) —' }, { value: 'true', label: 'true' }, { value: 'false', label: 'false' }] },
      { key: 'disable_write_with_if_match', label: 'Disable If-Match Writes', type: 'select', optional: true, half: true,
        options: [{ value: '', label: '— default (false) —' }, { value: 'true', label: 'true' }, { value: 'false', label: 'false' }] },
      { key: 'enable_write_with_append', label: 'Append Writes', type: 'select', optional: true, half: true,
        options: [{ value: '', label: '— default (false) —' }, { value: 'true', label: 'true' }, { value: 'false', label: 'false' }] },
      { divider: 'Limits' },
      { key: 'delete_max_size', label: 'Delete Max Size', type: 'text', placeholder: '1000', optional: true, half: true },
      { key: 'batch_max_operations', label: 'Batch Max Operations', type: 'text', placeholder: 'Deprecated; use delete max size', optional: true, half: true },
    ],
  },
  {
    label: 'Backblaze B2',
    scheme: 'b2',
    icon: 'hard-drive',
    description: 'Backblaze B2 cloud storage',
    rootPlaceholder: 'folder',
    fields: [
      { key: 'bucket', label: 'Bucket', type: 'text', placeholder: 'bucket-name', half: true },
      { key: 'bucket_id', label: 'Bucket ID', type: 'text', placeholder: '', half: true },
      { key: 'application_key_id', label: 'Application Key ID', type: 'text', placeholder: '', half: true },
      { key: 'application_key', label: 'Application Key', type: 'password', placeholder: '', half: true },
    ],
  },
  {
    label: 'Google Drive',
    scheme: 'gdrive',
    icon: 'cloud',
    description: 'Google Drive',
    oauth: true,
    rootPlaceholder: 'Work',
    fields: [
      { key: 'access_token', label: 'Access Token', type: 'password', placeholder: '' },
      { key: 'refresh_token', label: 'Refresh Token', type: 'password', placeholder: '', optional: true },
      { key: 'client_id', label: 'OAuth Client ID', type: 'text', placeholder: '', optional: true, half: true },
      { key: 'client_secret', label: 'Client Secret', type: 'password', placeholder: '', optional: true, half: true },
    ],
  },
  {
    label: 'OneDrive',
    scheme: 'onedrive',
    icon: 'cloud',
    description: 'Microsoft OneDrive',
    oauth: true,
    rootPlaceholder: 'Documents',
    fields: [
      { key: 'access_token', label: 'Access Token', type: 'password', placeholder: '' },
      { key: 'refresh_token', label: 'Refresh Token', type: 'password', placeholder: '', optional: true },
      { key: 'client_id', label: 'OAuth Client ID', type: 'text', placeholder: '', optional: true, half: true },
      { key: 'client_secret', label: 'Client Secret', type: 'password', placeholder: '', optional: true, half: true },
      { key: 'enable_versioning', label: 'Versioning', type: 'select', optional: true, half: true,
        options: [{ value: '', label: '— default (false) —' }, { value: 'true', label: 'true' }, { value: 'false', label: 'false' }] },
    ],
  },
  {
    label: 'Dropbox',
    scheme: 'dropbox',
    icon: 'package',
    description: 'Dropbox',
    oauth: true,
    rootPlaceholder: 'Projects',
    fields: [
      { key: 'access_token', label: 'Access Token', type: 'password', placeholder: '' },
      { key: 'refresh_token', label: 'Refresh Token', type: 'password', placeholder: '', optional: true },
      { key: 'client_id', label: 'OAuth Client ID', type: 'text', placeholder: '', optional: true, half: true },
      { key: 'client_secret', label: 'Client Secret', type: 'password', placeholder: '', optional: true, half: true },
    ],
  },
  {
    label: 'OpenStack Swift',
    scheme: 'swift',
    icon: 'layers',
    description: 'Rackspace Cloud Files / OpenStack Swift',
    rootPlaceholder: 'assets',
    fields: [
      { key: 'endpoint', label: 'Endpoint', type: 'text', placeholder: 'https://swift.example.com/v1/AUTH_abc' },
      { key: 'container', label: 'Container', type: 'text', placeholder: 'my-container' },
      { key: 'token', label: 'Auth Token', type: 'password', placeholder: 'Keystone token', optional: true },
      { divider: 'TempURL Signing' },
      { key: 'temp_url_key', label: 'TempURL Key', type: 'password', placeholder: 'X-Account-Meta-Temp-URL-Key value', optional: true },
      { key: 'temp_url_hash_algorithm', label: 'TempURL Hash', type: 'select', optional: true, half: true,
        options: [{ value: '', label: '— default (sha256) —' }, { value: 'sha256', label: 'SHA-256' }, { value: 'sha1', label: 'SHA-1' }, { value: 'sha512', label: 'SHA-512' }] },
    ],
  },
];

const initialProvider = providers.find((provider) => provider.scheme === NETWORK_LOCATION_SCHEME) ?? providers[0];
const visibleProviders = computed(() => providers.filter((provider) => {
  if (hiddenProviderSchemes.has(provider.scheme)) {
    return false;
  }

  return showAdvancedProviders.value || basicProviderSchemes.has(provider.scheme);
}));
const hasAdvancedProviders = computed(() => providers.some((provider) => (
  !hiddenProviderSchemes.has(provider.scheme)
  && !basicProviderSchemes.has(provider.scheme)
)));

function makeFields(provider) {
  const obj = {};
  for (const f of provider.fields) {
    if (!f.divider) obj[f.key] = f.default ?? '';
  }
  return obj;
}

const form = reactive({
  name: '',
  scheme: initialProvider.scheme,
  root: '',
  fields: makeFields(initialProvider),
});

const selectedProvider = computed(
  () => providers.find((p) => p.scheme === form.scheme) ?? initialProvider,
);
const isNetworkLocation = computed(() => selectedProvider.value.scheme === NETWORK_LOCATION_SCHEME);
const primaryFieldKey = computed(() => (isNetworkLocation.value ? 'host' : ''));
const selectedProviderSupportsOAuth = computed(() => Boolean(selectedProvider.value.oauth));
const selectedNetworkProtocol = computed(() => (
  form.fields.protocol || networkProtocolOptions[0].value
).toLowerCase());
const selectedNetworkKind = computed(() => (
  networkProtocolKinds[selectedNetworkProtocol.value] ?? 'sftp'
));
const isNetworkSmb = computed(() => selectedNetworkKind.value === 'smb');
const isNetworkSftp = computed(() => selectedNetworkKind.value === 'sftp');
const { isScrollable: remoteContentScrollable } = useScrollableContentState(remoteContent, {
  watch: [
    () => props.visible,
    () => form.scheme,
    () => form.fields.protocol,
    showAdvancedProviders,
  ],
});
const networkHostPlaceholder = computed(() => {
  switch (selectedNetworkKind.value) {
    case 'smb':
      return 'nas.local/share';
    case 'ftp':
      return 'ftp.example.com/public_html';
    case 'webdav':
      return 'cloud.example.com/remote.php/dav/files/user';
    default:
      return 'server.example.com/home/user';
  }
});

function setPrimaryFieldInput(el) {
  primaryFieldInput.value = el;
}

watch(
  () => form.scheme,
  (scheme) => {
    const provider = providers.find((p) => p.scheme === scheme) ?? initialProvider;
    form.fields = makeFields(provider);
    if (!form.name.trim()) form.name = provider.label;
    oauthMessage.value = '';
    errorMessage.value = '';
    primaryFieldInput.value = null;
    if (provider.scheme === NETWORK_LOCATION_SCHEME) {
      nextTick(() => primaryFieldInput.value?.focus());
    }
  },
);

watch(
  () => form.fields.protocol,
  () => {
    if (!isNetworkLocation.value) {
      return;
    }

    errorMessage.value = '';

    if (!isNetworkSmb.value) {
      form.fields.domain = '';
    }

    if (!isNetworkSftp.value) {
      form.fields.key = '';
      form.fields.known_hosts_strategy = '';
    }
  },
);

watch(
  () => props.visible,
  async (visible) => {
    if (!visible) {
      closeProtocolDropdown();
      return;
    }
    form.name = '';
    form.scheme = initialProvider.scheme;
    form.root = '';
    form.fields = makeFields(initialProvider);
    showAdvancedProviders.value = false;
    errorMessage.value = '';
    oauthMessage.value = '';
    primaryFieldInput.value = null;
    await nextTick();
    if (isNetworkLocation.value) {
      primaryFieldInput.value?.focus();
    } else {
      nameInput.value?.focus();
    }
  },
);

function slugify(value) {
  return value.toLowerCase().trim().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 42);
}

function createRemoteId(name, scheme) {
  return `${slugify(name) || scheme}-${Date.now().toString(36)}`;
}

function normalizeOptions(options) {
  return Object.fromEntries(
    Object.entries(options)
      .filter(([k, v]) => k.trim() && v !== null && v !== undefined && String(v).trim() !== '')
      .map(([k, v]) => [k.trim(), String(v)]),
  );
}

function decodeUrlValue(value) {
  if (!value) {
    return '';
  }

  try {
    return decodeURIComponent(value);
  } catch {
    return value;
  }
}

function normalizeNetworkRoot(pathname) {
  return decodeUrlValue(pathname || '')
    .replace(/^\/+/, '')
    .replace(/\/+$/, '');
}

function networkPathSegments(url) {
  return String(url.pathname || '')
    .split('/')
    .filter(Boolean);
}

function networkEndpoint(url, protocol, targetScheme) {
  if (targetScheme === 'sftp') {
    return url.host;
  }

  if (targetScheme === 'smb') {
    const share = networkPathSegments(url)[0] || '';
    return `smb://${url.host}/${share}`;
  }

  if (targetScheme === 'ftp') {
    const ftpProtocol = protocol === 'ftps' ? 'ftps' : 'ftp';
    return `${ftpProtocol}://${url.host}`;
  }

  const webdavProtocol = ['dav', 'http'].includes(protocol) ? 'http' : 'https';
  return `${webdavProtocol}://${url.host}`;
}

function networkAddressFromFields() {
  const host = (form.fields.host || '').trim();

  if (!host) {
    return '';
  }

  if (/^[a-z][a-z0-9+.-]*:\/\//i.test(host)) {
    return host;
  }

  const protocol = selectedNetworkProtocol.value || 'sftp';

  if (host.startsWith('//')) {
    return `${protocol}:${host}`;
  }

  return `${protocol}://${host}`;
}

function protocolOptionForUrlProtocol(protocol) {
  const aliases = {
    ssh: 'sftp',
    cifs: 'smb',
    webdav: 'davs',
    webdavs: 'davs',
    https: 'davs',
    http: 'dav',
  };
  const normalized = aliases[protocol] ?? protocol;

  return networkProtocolOptions.some((option) => option.value === normalized)
    ? normalized
    : '';
}

function normalizeNetworkHostField() {
  const host = (form.fields.host || '').trim();

  if (!host) {
    form.fields.host = '';
    return;
  }

  const match = host.match(/^([a-z][a-z0-9+.-]*):\/\//i);

  if (!match) {
    form.fields.host = host;
    return;
  }

  const protocol = protocolOptionForUrlProtocol(match[1].toLowerCase());

  if (!protocol) {
    return;
  }

  try {
    const url = new URL(host);
    const suffix = `${url.pathname || ''}${url.search || ''}${url.hash || ''}`;
    const username = decodeUrlValue(url.username);
    const password = decodeUrlValue(url.password || '');

    form.fields.protocol = protocol;
    form.fields.host = `${url.host}${suffix === '/' ? '' : suffix}`;

    if (username && !form.fields.username?.trim()) {
      form.fields.username = username;
    }

    if (password && !form.fields.password?.trim()) {
      form.fields.password = password;
    }
  } catch {
    form.fields.host = host;
  }
}

function networkVolumeName(url, targetScheme) {
  const rootName = (targetScheme === 'smb'
    ? decodeUrlValue(networkPathSegments(url)[0] || '')
    : normalizeNetworkRoot(url.pathname))
    .split('/')
    .filter(Boolean)
    .pop();

  return rootName || url.hostname || targetScheme.toUpperCase();
}

function parseNetworkLocation() {
  const address = networkAddressFromFields();

  if (!address) {
    return { error: 'Enter a server host or path.' };
  }

  let url;
  try {
    url = new URL(address);
  } catch {
    return { error: 'Enter a valid server address, for example server/path or nas/share.' };
  }

  const protocol = url.protocol.replace(/:$/, '').toLowerCase();
  const targetScheme = networkProtocolKinds[protocol];

  if (!targetScheme) {
    return { error: 'Supported network addresses are smb://, cifs://, sftp://, ssh://, ftp://, ftps://, dav://, davs://, webdav://, http://, and https://.' };
  }

  if (!url.hostname) {
    return { error: 'Server address must include a host.' };
  }

  const username = (form.fields.username || '').trim() || decodeUrlValue(url.username);
  const password = (form.fields.password || '').trim() || decodeUrlValue(url.password);
  const domain = (form.fields.domain || '').trim();
  const root = targetScheme === 'smb'
    ? normalizeNetworkRoot(`/${networkPathSegments(url).slice(1).join('/')}`)
    : normalizeNetworkRoot(url.pathname);
  const options = {
    endpoint: networkEndpoint(url, protocol, targetScheme),
  };

  if (targetScheme === 'smb' && !networkPathSegments(url)[0]) {
    return { error: 'SMB/CIFS addresses must include a share name, for example smb://server/share.' };
  }

  if (targetScheme === 'smb') {
    if (username) options.username = username;
    if (password) options.password = password;
    if (domain) options.domain = domain;
  } else if (targetScheme === 'sftp') {
    if (password && !username) {
      return { error: 'SFTP password authentication requires a username.' };
    }
    if (username) options.user = username;
    if (password) options.password = password;
  } else if (targetScheme === 'webdav') {
    if (username) options.username = username;
    if (password) options.password = password;
  } else {
    if (username) options.user = username;
    if (targetScheme === 'ftp' && password) options.password = password;
  }

  if (targetScheme === 'sftp') {
    const key = (form.fields.key || '').trim();
    const knownHostsStrategy = (form.fields.known_hosts_strategy || '').trim();

    if (key) options.key = key;
    if (knownHostsStrategy) options.known_hosts_strategy = knownHostsStrategy;
  }

  return {
    scheme: targetScheme,
    root: root || null,
    options: normalizeOptions(options),
    suggestedName: networkVolumeName(url, targetScheme),
  };
}

function buildOptions() {
  const options = {};
  for (const field of selectedProvider.value.fields) {
    if (field.divider) continue;
    const value = (form.fields[field.key] ?? '').trim();
    if (value) options[field.key] = value;
  }

  if (options._port) {
    if (options.endpoint) options.endpoint = `${options.endpoint}:${options._port}`;
    delete options._port;
  } else {
    delete options._port;
  }

  normalizeOAuthTokenMode(options);

  return normalizeOptions(options);
}

function normalizeOAuthTokenMode(options) {
  if (!selectedProvider.value.oauth) {
    return;
  }

  const hasAccessToken = Boolean(options.access_token);
  const hasRefreshToken = Boolean(options.refresh_token);

  if (!hasAccessToken || !hasRefreshToken) {
    return;
  }

  const canUseRefreshToken = Boolean(options.client_id)
    && (!oauthClientSecretRequired.has(form.scheme) || Boolean(options.client_secret));

  if (canUseRefreshToken) {
    delete options.access_token;
  } else {
    delete options.refresh_token;
  }
}

function validateOAuthOptions(options) {
  if (!selectedProvider.value.oauth) {
    return '';
  }

  if (!options.access_token && !options.refresh_token) {
    return `Create or paste a ${selectedProvider.value.label} access token before connecting.`;
  }

  if (!options.refresh_token) {
    return '';
  }

  if (!options.client_id) {
    return 'OAuth client ID is required when using a refresh token.';
  }

  if (oauthClientSecretRequired.has(form.scheme) && !options.client_secret) {
    return `${selectedProvider.value.label} requires the client secret when connecting with a refresh token.`;
  }

  return '';
}

async function createProviderTokens() {
  const provider = selectedProvider.value;

  if (!provider.oauth || creatingTokens.value) {
    return;
  }

  const clientId = form.fields.client_id?.trim();
  const clientSecret = form.fields.client_secret?.trim();

  if (!clientId) {
    errorMessage.value = 'Enter the OAuth client ID first.';
    return;
  }

  creatingTokens.value = true;
  errorMessage.value = '';
  oauthMessage.value = 'Waiting for browser authorization...';

  try {
    const tokens = await createOAuthTokens(form.scheme, clientId, clientSecret);
    const hasUsableRefreshToken = Boolean(tokens.refreshToken)
      && (!oauthClientSecretRequired.has(form.scheme) || Boolean(clientSecret));

    if (hasUsableRefreshToken) {
      form.fields.refresh_token = tokens.refreshToken;
      form.fields.access_token = '';
      oauthMessage.value = 'Refresh token created. OpenDAL will refresh access automatically.';
    } else {
      form.fields.access_token = tokens.accessToken;
      if (tokens.refreshToken) {
        form.fields.refresh_token = tokens.refreshToken;
      }
      oauthMessage.value = tokens.refreshToken
        ? 'Access token created. Add the client secret before connecting to use the refresh token.'
        : 'Access token created.';
    }
  } catch (error) {
    errorMessage.value = error?.message || 'Unable to create OAuth tokens.';
    oauthMessage.value = '';
  } finally {
    creatingTokens.value = false;
  }
}

function close(event) {
  event?.preventDefault?.();
  event?.stopPropagation?.();
  emit('close');
}

function handleKeydown(event) {
  if (event.key === 'Escape') {
    event.preventDefault();
    close();
  }
}

async function submit() {
  let name = form.name.trim();

  if (!name && !isNetworkLocation.value) {
    errorMessage.value = 'Please enter a name for this connection.';
    nameInput.value?.focus();
    return;
  }

  errorMessage.value = '';
  let scheme = form.scheme;
  let root = form.root.trim() || null;
  let options = {};

  if (isNetworkLocation.value) {
    const network = parseNetworkLocation();

    if (network.error) {
      errorMessage.value = network.error;
      primaryFieldInput.value?.focus();
      return;
    }

    name ||= network.suggestedName;
    scheme = network.scheme;
    root = network.root;
    options = network.options;
  } else {
    options = buildOptions();
  }

  const optionsError = validateOAuthOptions(options);

  if (optionsError) {
    errorMessage.value = optionsError;
    return;
  }

  saving.value = true;

  try {
    const remote = await addRemoteVolume({
      id: createRemoteId(name, scheme),
      name,
      scheme,
      root,
      options,
    });

    await store.refreshVolumes();
    store.setPanePath(store.activePaneId, remote.path);
    emit('close');
  } catch (error) {
    errorMessage.value = error?.message || 'Remote connection failed.';
  } finally {
    saving.value = false;
  }
}
</script>

<template>
  <Teleport to="body">
    <Transition name="remote-modal">
      <div
        v-if="visible"
        class="remote-overlay"
        role="presentation"
        @pointerdown.self="close"
        @keydown.stop="handleKeydown"
      >
        <section
          class="remote-panel"
          :class="{ 'remote-panel--content-scrollable': remoteContentScrollable }"
          role="dialog"
          aria-modal="true"
          aria-labelledby="remote-volume-title"
          tabindex="-1"
        >
          <!-- Header -->
          <header class="remote-header">
            <div class="remote-header-icon" aria-hidden="true">
              <AppIcon name="network" :size="18" :stroke-width="1.9" />
            </div>
            <h2 id="remote-volume-title">Connect Remote Volume</h2>
            <button type="button" class="remote-close" aria-label="Close" @click.stop="close">
              <AppIcon name="x" :size="14" :stroke-width="2" />
            </button>
          </header>

          <form class="remote-form" @submit.prevent="submit">
            <div ref="remoteContent" class="remote-content">
              <!-- Protocol picker -->
              <div class="remote-section">
                <div class="remote-section-heading">
                  <p class="remote-section-label">Connection</p>
                  <button
                    v-if="hasAdvancedProviders"
                    type="button"
                    class="provider-advanced-toggle"
                    :class="{ 'provider-advanced-toggle--active': showAdvancedProviders }"
                    @click="toggleAdvancedProviders"
                  >
                    <AppIcon name="sliders" :size="13" :stroke-width="2" />
                    Advanced forms
                  </button>
                </div>
                <div ref="protocolTriggerRef" class="protocol-select">
                  <button
                    type="button"
                    class="protocol-trigger"
                    :class="{ 'protocol-trigger--open': protocolDropdownOpen }"
                    aria-haspopup="listbox"
                    :aria-expanded="protocolDropdownOpen"
                    @click="toggleProtocolDropdown"
                  >
                    <span class="protocol-trigger-icon" aria-hidden="true">
                      <AppIcon :name="selectedProvider.icon" :size="17" :stroke-width="1.8" />
                    </span>
                    <span class="protocol-trigger-body">
                      <span class="protocol-trigger-label">{{ selectedProvider.label }}</span>
                      <span class="protocol-trigger-desc">{{ selectedProvider.description }}</span>
                    </span>
                    <span class="protocol-trigger-chevron" aria-hidden="true">
                      <AppIcon name="chevron-down" :size="14" :stroke-width="2.2" />
                    </span>
                  </button>
                </div>

                <Teleport to="body">
                  <div
                    v-if="protocolDropdownOpen"
                    ref="protocolDropdownRef"
                    class="protocol-dropdown"
                    role="listbox"
                    :style="{
                      top: `${dropdownRect.top}px`,
                      left: `${dropdownRect.left}px`,
                      width: `${dropdownRect.width}px`,
                    }"
                  >
                    <button
                      v-for="provider in visibleProviders"
                      :key="provider.scheme"
                      type="button"
                      class="protocol-option"
                      :class="{ 'protocol-option--active': form.scheme === provider.scheme }"
                      role="option"
                      :aria-selected="form.scheme === provider.scheme"
                      @click="selectProtocol(provider.scheme)"
                    >
                      <span class="protocol-option-icon" aria-hidden="true">
                        <AppIcon :name="provider.icon" :size="16" :stroke-width="1.8" />
                      </span>
                      <span class="protocol-option-body">
                        <span class="protocol-option-label">{{ provider.label }}</span>
                        <span class="protocol-option-desc">{{ provider.description }}</span>
                      </span>
                      <span v-if="form.scheme === provider.scheme" class="protocol-option-check" aria-hidden="true">
                        <AppIcon name="check" :size="13" :stroke-width="2.6" />
                      </span>
                    </button>
                  </div>
                </Teleport>
              </div>

              <!-- Name + Root -->
              <div class="remote-section remote-row" :class="{ 'remote-row--single': isNetworkLocation }">
                <label class="remote-field">
                  <span>Name <em v-if="isNetworkLocation">(optional)</em></span>
                  <input
                    ref="nameInput"
                    v-model="form.name"
                    type="text"
                    autocomplete="off"
                    spellcheck="false"
                    :placeholder="isNetworkLocation ? 'Derived from address' : 'Production server'"
                  />
                </label>
                <label v-if="!isNetworkLocation" class="remote-field">
                  <span>Root path <em>(optional)</em></span>
                  <input
                    v-model="form.root"
                    type="text"
                    autocomplete="off"
                    spellcheck="false"
                    :placeholder="selectedProvider.rootPlaceholder"
                  />
                </label>
              </div>

              <!-- Provider-specific credentials -->
              <div class="remote-section">
                <p class="remote-section-label">{{ isNetworkLocation ? 'Details' : 'Credentials' }}</p>
                <div v-if="selectedProviderSupportsOAuth" class="oauth-helper">
                  <div class="oauth-helper-copy">
                    <strong>OAuth</strong>
                    <span>Redirect URI: {{ OAUTH_CALLBACK_URL }}</span>
                  </div>
                  <button
                    type="button"
                    class="oauth-helper-button"
                    :disabled="creatingTokens || saving"
                    @click="createProviderTokens"
                  >
                    <AppIcon name="lock" :size="13" :stroke-width="2" />
                    {{ creatingTokens ? 'Waiting...' : 'Create token' }}
                  </button>
                </div>
                <p v-if="oauthMessage" class="oauth-message">
                  {{ oauthMessage }}
                </p>
                <div v-if="isNetworkLocation" class="network-fields">
                  <label class="remote-field">
                    <span>Server</span>
                    <div class="server-address-control">
                      <select v-model="form.fields.protocol" aria-label="Protocol">
                        <option
                          v-for="option in networkProtocolOptions"
                          :key="option.value"
                          :value="option.value"
                        >
                          {{ option.label }}
                        </option>
                      </select>
                      <input
                        ref="primaryFieldInput"
                        v-model="form.fields.host"
                        type="text"
                        autocomplete="off"
                        spellcheck="false"
                        :placeholder="networkHostPlaceholder"
                        aria-label="Server host and path"
                        @blur="normalizeNetworkHostField"
                      />
                    </div>
                  </label>

                  <div class="creds-grid">
                    <label class="remote-field remote-field--half">
                      <span>Username <em>(optional)</em></span>
                      <input
                        v-model="form.fields.username"
                        type="text"
                        autocomplete="off"
                        spellcheck="false"
                      />
                    </label>
                    <label class="remote-field remote-field--half">
                      <span>Password <em>(optional)</em></span>
                      <input
                        v-model="form.fields.password"
                        type="password"
                        autocomplete="off"
                        spellcheck="false"
                      />
                    </label>
                    <label v-if="isNetworkSmb" class="remote-field remote-field--half">
                      <span>Domain / Workgroup <em>(optional)</em></span>
                      <input
                        v-model="form.fields.domain"
                        type="text"
                        autocomplete="off"
                        spellcheck="false"
                        placeholder="WORKGROUP"
                      />
                    </label>
                    <label v-if="isNetworkSftp" class="remote-field">
                      <span>SSH Private Key / Path <em>(optional)</em></span>
                      <input
                        v-model="form.fields.key"
                        type="text"
                        autocomplete="off"
                        spellcheck="false"
                        placeholder="~/.ssh/id_rsa"
                      />
                    </label>
                    <label v-if="isNetworkSftp" class="remote-field remote-field--half">
                      <span>SSH Known Hosts <em>(optional)</em></span>
                      <select v-model="form.fields.known_hosts_strategy">
                        <option value="">— default (strict) —</option>
                        <option value="strict">Strict</option>
                        <option value="accept">Accept all</option>
                        <option value="add">Add & trust</option>
                      </select>
                    </label>
                  </div>
                </div>
                <div v-else class="creds-grid">
                  <template v-for="field in selectedProvider.fields" :key="field.divider ?? field.key">
                    <!-- Section divider -->
                    <div v-if="field.divider" class="creds-divider">{{ field.divider }}</div>

                    <!-- Select field -->
                    <label
                      v-else-if="field.type === 'select'"
                      class="remote-field"
                      :class="{ 'remote-field--half': field.half }"
                    >
                      <span>{{ field.label }} <em v-if="field.optional">(optional)</em></span>
                      <select v-model="form.fields[field.key]">
                        <option v-for="opt in field.options" :key="opt.value" :value="opt.value">
                          {{ opt.label }}
                        </option>
                      </select>
                    </label>

                    <!-- Text / password field -->
                    <label
                      v-else
                      class="remote-field"
                      :class="{ 'remote-field--half': field.half }"
                    >
                      <span>{{ field.label }} <em v-if="field.optional">(optional)</em></span>
                      <input
                        :ref="field.key === primaryFieldKey ? setPrimaryFieldInput : null"
                        v-model="form.fields[field.key]"
                        :type="field.type"
                        autocomplete="off"
                        spellcheck="false"
                        :placeholder="field.placeholder"
                      />
                    </label>
                  </template>
                </div>
              </div>
            </div>

            <footer class="remote-footer">
              <p v-if="errorMessage" class="remote-error" role="alert">
                <AppIcon name="alert" :size="14" :stroke-width="2" />
                {{ errorMessage }}
              </p>

              <div class="remote-actions">
                <button type="button" class="app-button" :disabled="saving" @click.stop="close">
                  Cancel
                </button>
                <button type="submit" class="app-button app-button--primary" :disabled="saving">
                  {{ saving ? 'Connecting…' : 'Connect' }}
                </button>
              </div>
            </footer>
          </form>
        </section>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.remote-overlay {
  position: fixed;
  inset: 0;
  z-index: 80;
  display: grid;
  place-items: center;
  padding: 28px;
  background: var(--overlay-bg);
}

.remote-panel {
  display: flex;
  width: min(520px, 100%);
  max-height: min(780px, calc(100vh - 56px));
  flex-direction: column;
  overflow: hidden;
  border: 1px solid var(--control-border);
  border-radius: var(--radius-panel);
  background: var(--popover-bg);
  box-shadow: var(--shadow-overlay);
  color: var(--text);
  outline: none;
}

/* ── Header ───────────────────────────────────────────────── */
.remote-header {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-shrink: 0;
  padding: 16px 16px 16px 18px;
  border-bottom: 1px solid transparent;
}

.remote-panel--content-scrollable .remote-header {
  border-bottom-color: var(--hairline);
}

.remote-header-icon {
  display: grid;
  width: 30px;
  height: 30px;
  place-items: center;
  border-radius: 8px;
  background: rgb(var(--accent-rgb) / 0.12);
  color: var(--accent);
  flex-shrink: 0;
}

.remote-header h2 {
  flex: 1;
  margin: 0;
  font-size: 14px;
  font-weight: 680;
  letter-spacing: -0.01em;
}

.remote-close {
  display: grid;
  place-items: center;
  width: 26px;
  height: 26px;
  border-radius: 7px;
  background: transparent;
  color: var(--icon);
  transition: background 100ms ease, color 100ms ease;
}

.remote-close:hover {
  background: var(--btn-hover);
  color: var(--text);
}

/* ── Form body ────────────────────────────────────────────── */
.remote-form {
  display: flex;
  flex: 1;
  flex-direction: column;
  min-height: 0;
  overflow: hidden;
}

.remote-content {
  display: grid;
  flex: 1;
  min-height: 0;
  gap: 16px;
  overflow-y: auto;
  padding: 16px 18px;
}

.remote-section-label {
  margin: 0 0 8px;
  color: var(--text-faint);
  font-size: 10.5px;
  font-weight: 700;
  letter-spacing: 0.04em;
  text-transform: uppercase;
}

.remote-section-heading {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  margin-bottom: 8px;
}

.remote-section-heading .remote-section-label {
  margin: 0;
}

.provider-advanced-toggle {
  display: inline-flex;
  height: 24px;
  align-items: center;
  justify-content: center;
  gap: 5px;
  border: 1px solid transparent;
  border-radius: 7px;
  padding: 0 8px;
  background: transparent;
  color: var(--text-faint);
  font-size: 11px;
  font-weight: 650;
  transition: background 90ms ease, border-color 90ms ease, color 90ms ease;
}

.provider-advanced-toggle:hover {
  background: var(--btn-hover);
  color: var(--text-muted);
}

.provider-advanced-toggle--active {
  border-color: rgb(var(--accent-rgb) / 0.22);
  background: rgb(var(--accent-rgb) / 0.10);
  color: var(--accent);
}

/* ── Protocol custom select ───────────────────────────────── */
.protocol-select {
  position: relative;
}

.protocol-trigger {
  display: flex;
  align-items: center;
  gap: 10px;
  width: 100%;
  height: 48px;
  padding: 0 12px 0 14px;
  border: 1px solid var(--input-border);
  border-radius: 9px;
  background: var(--input-bg);
  box-shadow: var(--input-shadow);
  color: var(--text);
  text-align: left;
  transition: border-color 120ms ease, box-shadow 120ms ease;
}

.protocol-trigger:hover {
  border-color: var(--control-border);
}

.protocol-trigger--open {
  border-color: var(--accent-border);
  box-shadow: var(--accent-focus-ring), var(--input-shadow);
}

.protocol-trigger-icon {
  display: flex;
  flex-shrink: 0;
  color: var(--accent);
}

.protocol-trigger-body {
  display: flex;
  flex-direction: column;
  gap: 2px;
  flex: 1;
  min-width: 0;
}

.protocol-trigger-label {
  font-size: 13px;
  font-weight: 600;
  line-height: 1;
}

.protocol-trigger-desc {
  font-size: 11px;
  color: var(--text-faint);
  line-height: 1;
}

.protocol-trigger-chevron {
  display: flex;
  flex-shrink: 0;
  color: var(--icon);
  transition: transform 160ms cubic-bezier(0.2, 0, 0, 1);
}

.protocol-trigger--open .protocol-trigger-chevron {
  transform: rotate(180deg);
}

/* ── Protocol dropdown ────────────────────────────────────── */
.protocol-dropdown {
  position: fixed;
  z-index: 9000;
  overflow-y: auto;
  max-height: 320px;
  padding: 4px;
  border: 1px solid var(--control-border);
  border-radius: 11px;
  background: var(--popover-bg);
  box-shadow: var(--shadow-overlay);
  animation: protocol-dropdown-in 130ms cubic-bezier(0.2, 0, 0, 1) forwards;
}

@keyframes protocol-dropdown-in {
  from {
    opacity: 0;
    transform: translateY(-5px) scale(0.98);
    transform-origin: top center;
  }
  to {
    opacity: 1;
    transform: none;
  }
}

.protocol-option {
  display: flex;
  align-items: center;
  gap: 10px;
  width: 100%;
  padding: 9px 8px;
  border-radius: 7px;
  background: transparent;
  color: var(--text);
  text-align: left;
  transition: background 80ms ease;
}

.protocol-option:hover {
  background: var(--btn-hover);
}

.protocol-option--active {
  background: rgb(var(--accent-rgb) / 0.08);
}

.protocol-option-icon {
  display: flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
  width: 30px;
  height: 30px;
  border-radius: 8px;
  background: rgb(var(--accent-rgb) / 0.10);
  color: var(--accent);
}

.protocol-option--active .protocol-option-icon {
  background: rgb(var(--accent-rgb) / 0.16);
}

.protocol-option-body {
  display: flex;
  flex-direction: column;
  gap: 2px;
  flex: 1;
  min-width: 0;
}

.protocol-option-label {
  font-size: 13px;
  font-weight: 580;
  line-height: 1;
}

.protocol-option-desc {
  font-size: 11px;
  color: var(--text-faint);
  line-height: 1;
}

.protocol-option-check {
  display: flex;
  flex-shrink: 0;
  color: var(--accent);
}

/* ── OAuth helper ─────────────────────────────────────────── */
.oauth-helper {
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto;
  align-items: center;
  gap: 10px;
  margin-bottom: 10px;
  border: 1px solid var(--control-border);
  border-radius: 8px;
  padding: 9px 10px;
  background: var(--control-bg);
  box-shadow: var(--control-inset);
}

.oauth-helper-copy {
  display: grid;
  min-width: 0;
  gap: 2px;
}

.oauth-helper-copy strong {
  color: var(--text);
  font-size: 12.5px;
  font-weight: 700;
}

.oauth-helper-copy span,
.oauth-message {
  overflow: hidden;
  color: var(--text-faint);
  font-size: 11px;
  font-weight: 560;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.oauth-helper-button {
  display: inline-flex;
  height: 28px;
  align-items: center;
  justify-content: center;
  gap: 5px;
  border: 1px solid var(--control-border);
  border-radius: 7px;
  padding: 0 9px;
  background: var(--btn-hover);
  color: var(--text-muted);
  font-size: 12px;
  font-weight: 650;
  transition: background 90ms ease, color 90ms ease, opacity 90ms ease;
}

.oauth-helper-button:hover:not(:disabled) {
  background: var(--btn-active-bg);
  color: var(--text);
}

.oauth-helper-button:disabled {
  cursor: default;
  opacity: 0.55;
}

.oauth-message {
  margin: -2px 0 10px;
  white-space: normal;
}

/* ── Name + Root row ──────────────────────────────────────── */
.remote-row {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 10px;
}

.remote-row--single {
  grid-template-columns: 1fr;
}

/* ── Credential fields ────────────────────────────────────── */
.creds-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 10px;
}

.creds-divider {
  grid-column: 1 / -1;
  margin: 4px 0 0;
  padding-top: 10px;
  border-top: 1px solid var(--hairline);
  color: var(--text-faint);
  font-size: 10.5px;
  font-weight: 700;
  letter-spacing: 0.04em;
  text-transform: uppercase;
}

.remote-field {
  display: grid;
  gap: 6px;
  grid-column: 1 / -1;
}

.remote-field--half {
  grid-column: auto;
}

.remote-field span {
  color: var(--text-muted);
  font-size: 11.5px;
  font-weight: 650;
}

.remote-field span em {
  font-style: normal;
  font-weight: 460;
  color: var(--text-faint);
}

.remote-field input,
.remote-field select {
  width: 100%;
  min-width: 0;
  height: 34px;
  border: 1px solid var(--input-border);
  border-radius: 8px;
  padding: 0 10px;
  background: var(--input-bg);
  color: var(--text);
  font: inherit;
  font-size: 13px;
  outline: none;
  box-shadow: var(--input-shadow);
  transition: border-color 120ms ease, box-shadow 120ms ease;
}

.remote-field select {
  appearance: none;
  -webkit-appearance: none;
  background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='12' height='12' viewBox='0 0 24 24' fill='none' stroke='%23888' stroke-width='2.2' stroke-linecap='round' stroke-linejoin='round'%3E%3Cpolyline points='6 9 12 15 18 9'/%3E%3C/svg%3E");
  background-repeat: no-repeat;
  background-position: right 10px center;
  padding-right: 30px;
}

.remote-field input:focus,
.remote-field select:focus {
  border-color: var(--accent-border);
  box-shadow:
    var(--accent-focus-ring),
    var(--input-shadow);
}

.network-fields {
  display: grid;
  gap: 10px;
}

.server-address-control {
  display: grid;
  grid-template-columns: minmax(112px, 132px) minmax(0, 1fr);
  overflow: hidden;
  height: 36px;
  border: 1px solid var(--input-border);
  border-radius: 9px;
  background: var(--input-bg);
  box-shadow: var(--input-shadow);
  transition: border-color 120ms ease, box-shadow 120ms ease;
}

.server-address-control:focus-within {
  border-color: var(--accent-border);
  box-shadow:
    var(--accent-focus-ring),
    var(--input-shadow);
}

.server-address-control select,
.server-address-control input {
  height: 100%;
  border: 0;
  border-radius: 0;
  background-color: transparent;
  box-shadow: none;
}

.server-address-control select {
  border-right: 1px solid var(--hairline);
  background-position: right 8px center;
  color: var(--text);
  font-weight: 700;
}

.server-address-control input {
  padding-left: 12px;
}

.server-address-control select:focus,
.server-address-control input:focus {
  border-color: transparent;
  box-shadow: none;
}

@media (max-width: 460px) {
  .server-address-control {
    grid-template-columns: minmax(92px, 108px) minmax(0, 1fr);
  }
}

/* ── Error ────────────────────────────────────────────────── */
.remote-error {
  display: flex;
  align-items: center;
  gap: 7px;
  margin: 0;
  border: 1px solid rgb(var(--danger-rgb) / 0.22);
  border-radius: 8px;
  padding: 9px 11px;
  background: rgb(var(--danger-rgb) / 0.09);
  color: var(--danger);
  font-size: 12.5px;
  line-height: 1.35;
}

/* ── Actions ──────────────────────────────────────────────── */
.remote-footer {
  display: grid;
  flex: 0 0 auto;
  gap: 10px;
  border-top: 1px solid transparent;
  padding: 12px 18px 14px;
}

.remote-panel--content-scrollable .remote-footer {
  border-top-color: var(--hairline);
}

.remote-actions {
  display: flex;
  justify-content: flex-end;
  gap: 10px;
}

/* ── Animation ────────────────────────────────────────────── */
.remote-modal-enter-active {
  transition: opacity 180ms ease;
}

.remote-modal-leave-active {
  transition: opacity 120ms ease;
}

.remote-modal-enter-active .remote-panel {
  transition: transform 200ms cubic-bezier(0.2, 0, 0, 1), opacity 180ms ease;
}

.remote-modal-leave-active .remote-panel {
  transition: transform 120ms ease, opacity 120ms ease;
}

.remote-modal-enter-from,
.remote-modal-leave-to {
  opacity: 0;
}

.remote-modal-enter-from .remote-panel,
.remote-modal-leave-to .remote-panel {
  opacity: 0;
  transform: scale(0.98) translateY(8px);
}
</style>
