export type GuardPolicy = Record<string, unknown>

export type NodeSecurity = {
  id: string
  name: string
  guard_policy: GuardPolicy
  effective_policy: GuardPolicy
  has_overlay: boolean
  overlay_enabled: boolean
  trusted_proxies: string[]
}

export type PolicyForm = {
  tcpNewPerSec: string
  udpPps: string
  udpNewFlowsPerSec: string
  maxTunnels: string
  maxDistinctIps: string
  blockHttpOnL4: boolean
  perIpLimitEnabled: boolean
  udpFloodEnabled: boolean
  nodeLimitsEnabled: boolean
  perTunnelIpEnabled: boolean
  onAttackEnabled: boolean
  onAttackPauseMinutes: string
  onAttackMinHits: string
  cnResidencyEnabled: boolean
  cnHttpFilingEnabled: boolean
  ipAclEnabled: boolean
  ipAllow: string
  ipDeny: string
  trustedProxies: string
}

export const EMPTY_FORM: PolicyForm = {
  tcpNewPerSec: '64',
  udpPps: '200000',
  udpNewFlowsPerSec: '128',
  maxTunnels: '0',
  maxDistinctIps: '64',
  blockHttpOnL4: false,
  perIpLimitEnabled: true,
  udpFloodEnabled: true,
  nodeLimitsEnabled: true,
  perTunnelIpEnabled: false,
  onAttackEnabled: false,
  onAttackPauseMinutes: '0',
  onAttackMinHits: '32',
  cnResidencyEnabled: false,
  cnHttpFilingEnabled: false,
  ipAclEnabled: false,
  ipAllow: '',
  ipDeny: '',
  trustedProxies: '',
}

export function moduleEnabled(policy: GuardPolicy, key: string, fallback = true): boolean {
  const mod = policy[key]
  if (!mod || typeof mod !== 'object' || Array.isArray(mod)) return fallback
  const enabled = (mod as Record<string, unknown>).enabled
  return typeof enabled === 'boolean' ? enabled : fallback
}

export function moduleNumber(policy: GuardPolicy, key: string, field: string, fallback: number): number {
  const mod = policy[key]
  if (!mod || typeof mod !== 'object' || Array.isArray(mod)) return fallback
  const value = (mod as Record<string, unknown>)[field]
  return typeof value === 'number' && Number.isFinite(value) ? value : fallback
}

function moduleCidrs(policy: GuardPolicy, key: string, field: string): string {
  const mod = policy[key]
  if (!mod || typeof mod !== 'object' || Array.isArray(mod)) return ''
  const value = (mod as Record<string, unknown>)[field]
  if (!Array.isArray(value)) return ''
  return value.filter((item): item is string => typeof item === 'string').join('\n')
}

export function policyToForm(policy: GuardPolicy, trustedProxies: string[] = []): PolicyForm {
  return {
    tcpNewPerSec: String(moduleNumber(policy, 'per_ip_limit', 'max_new_per_sec', 64)),
    udpPps: String(moduleNumber(policy, 'udp_amplify', 'max_packets_per_sec', 200000)),
    udpNewFlowsPerSec: String(moduleNumber(policy, 'udp_amplify', 'max_new_flows_per_sec', 128)),
    maxTunnels: String(moduleNumber(policy, 'node_limits', 'max_tunnels', 0)),
    maxDistinctIps: String(moduleNumber(policy, 'per_tunnel_ip_limit', 'max_distinct_ips', 64)),
    blockHttpOnL4: moduleEnabled(policy, 'block_http_on_l4', false),
    perIpLimitEnabled: moduleEnabled(policy, 'per_ip_limit', true),
    udpFloodEnabled: moduleEnabled(policy, 'udp_amplify', true),
    nodeLimitsEnabled: moduleEnabled(policy, 'node_limits', true),
    perTunnelIpEnabled: moduleEnabled(policy, 'per_tunnel_ip_limit', false),
    onAttackEnabled: moduleEnabled(policy, 'on_attack', false),
    onAttackPauseMinutes: String(moduleNumber(policy, 'on_attack', 'pause_minutes', 0)),
    onAttackMinHits: String(moduleNumber(policy, 'on_attack', 'min_hits', 32)),
    cnResidencyEnabled: moduleEnabled(policy, 'cn_residency', false),
    cnHttpFilingEnabled: moduleEnabled(policy, 'cn_http_filing', false),
    ipAclEnabled: moduleEnabled(policy, 'ip_acl', false),
    ipAllow: moduleCidrs(policy, 'ip_acl', 'allow'),
    ipDeny: moduleCidrs(policy, 'ip_acl', 'deny'),
    trustedProxies: trustedProxies.join('\n'),
  }
}

export function parseCidrLines(raw: string): string[] {
  return raw.split(/[\s,，]+/).map((value) => value.trim()).filter(Boolean)
}

export function formToPolicy(form: PolicyForm, options?: { asNodeOverlay?: boolean; overlayEnabled?: boolean }): GuardPolicy {
  const policy: GuardPolicy = {
    per_ip_limit: {
      enabled: form.perIpLimitEnabled,
      max_new_per_sec: Number(form.tcpNewPerSec) || 0,
    },
    udp_amplify: {
      enabled: form.udpFloodEnabled,
      max_packets_per_sec: Number(form.udpPps) || 0,
      max_new_flows_per_sec: Number(form.udpNewFlowsPerSec) || 0,
    },
    node_limits: {
      enabled: form.nodeLimitsEnabled,
      max_tunnels: Number(form.maxTunnels) || 0,
    },
    block_http_on_l4: {
      enabled: form.blockHttpOnL4,
    },
    per_tunnel_ip_limit: {
      enabled: form.perTunnelIpEnabled,
      max_distinct_ips: Number(form.maxDistinctIps) || 0,
    },
    on_attack: {
      enabled: form.onAttackEnabled,
      pause_minutes: Math.max(0, Number(form.onAttackPauseMinutes) || 0),
      min_hits: Math.max(1, Number(form.onAttackMinHits) || 32),
    },
    cn_residency: {
      enabled: form.cnResidencyEnabled,
    },
    cn_http_filing: {
      enabled: form.cnHttpFilingEnabled,
    },
    ip_acl: {
      enabled: form.ipAclEnabled,
      allow: parseCidrLines(form.ipAllow),
      deny: parseCidrLines(form.ipDeny),
    },
    http_guard: { enabled: true, max_header_bytes: 16384, max_headers: 100 },
    auto_ban: { enabled: true },
    tls_guard: { enabled: true },
    preauth_guard: { enabled: true },
  }
  if (options?.asNodeOverlay) {
    policy.overlay = { enabled: options.overlayEnabled !== false }
  }
  return policy
}

export function withOverlayEnabled(policy: GuardPolicy, enabled: boolean): GuardPolicy {
  return {
    ...policy,
    overlay: { enabled },
  }
}

export function summaryParts(policy: GuardPolicy, t: (key: string, opts?: Record<string, unknown>) => string): string[] {
  const parts: string[] = []
  if (moduleEnabled(policy, 'per_ip_limit')) {
    parts.push(t('security.summary.tcpRate', { n: moduleNumber(policy, 'per_ip_limit', 'max_new_per_sec', 64) }))
  }
  if (moduleEnabled(policy, 'udp_amplify')) {
    parts.push(t('security.summary.udpPps', { n: moduleNumber(policy, 'udp_amplify', 'max_packets_per_sec', 200000) }))
  }
  if (moduleEnabled(policy, 'node_limits')) {
    const max = moduleNumber(policy, 'node_limits', 'max_tunnels', 0)
    parts.push(max > 0 ? t('security.summary.maxTunnels', { n: max }) : t('security.summary.tunnelsUnlimited'))
  }
  if (moduleEnabled(policy, 'block_http_on_l4', false)) {
    parts.push(t('security.summary.blockHttpOnL4'))
  }
  if (moduleEnabled(policy, 'per_tunnel_ip_limit', false)) {
    parts.push(t('security.summary.maxDistinctIps', { n: moduleNumber(policy, 'per_tunnel_ip_limit', 'max_distinct_ips', 64) }))
  }
  if (moduleEnabled(policy, 'on_attack', false)) {
    const mins = moduleNumber(policy, 'on_attack', 'pause_minutes', 0)
    parts.push(mins > 0 ? t('security.summary.onAttackTimed', { n: mins }) : t('security.summary.onAttackManual'))
  }
  if (moduleEnabled(policy, 'cn_residency', false)) {
    parts.push(t('security.summary.cnResidency'))
  }
  if (moduleEnabled(policy, 'cn_http_filing', false)) {
    parts.push(t('security.summary.cnHttpFiling'))
  }
  if (moduleEnabled(policy, 'ip_acl', false)) {
    parts.push(t('security.summary.ipAcl'))
  }
  return parts
}

export function translateGuardRule(t: (key: string, opts?: Record<string, unknown>) => string, rule: string): string {
  const key = `security.rules.${rule}`
  const translated = t(key)
  return translated === key ? rule : translated
}
