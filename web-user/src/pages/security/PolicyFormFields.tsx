import { useTranslation } from 'react-i18next'
import { FormField, FormStack } from '@/components/FormField'
import { Input } from '@/components/Input'
import { translateField } from '@/i18n/fieldLabel'
import type { PolicyForm } from './policyShared'
import { PolicyLabel, PolicySection } from './PolicyLabel'

export function PolicyFormFields({
  form,
  setForm,
  showTrustedProxies,
}: {
  form: PolicyForm
  setForm: (next: PolicyForm) => void
  showTrustedProxies?: boolean
}) {
  const { t } = useTranslation()
  const patch = <K extends keyof PolicyForm>(key: K, value: PolicyForm[K]) => {
    setForm({ ...form, [key]: value })
  }

  return (
    <FormStack>
      <PolicySection title={t('security.categories.rateLimit')}>
        <label className="flex items-center gap-2 text-sm">
          <input type="checkbox" checked={form.perIpLimitEnabled} onChange={(e) => patch('perIpLimitEnabled', e.target.checked)} />
          <PolicyLabel name={t('security.policies.tcpRate.name')} tip={t('security.policies.tcpRate.tip')} />
        </label>
        <FormField label={t('security.tcpNew')}>
          <Input type="number" min={0} max={1000000} value={form.tcpNew} onChange={(e) => patch('tcpNew', e.target.value)} disabled={!form.perIpLimitEnabled} />
        </FormField>
        <FormField label={t('security.tcpWindowSecs')}>
          <Input type="number" min={1} max={86400} value={form.tcpWindowSecs} onChange={(e) => patch('tcpWindowSecs', e.target.value)} disabled={!form.perIpLimitEnabled} />
        </FormField>

        <label className="flex items-center gap-2 text-sm">
          <input type="checkbox" checked={form.udpFloodEnabled} onChange={(e) => patch('udpFloodEnabled', e.target.checked)} />
          <PolicyLabel name={t('security.policies.udpFlood.name')} tip={t('security.policies.udpFlood.tip')} />
        </label>
        <FormField label={t('security.udpPps')}>
          <Input type="number" min={0} max={1000000} value={form.udpPps} onChange={(e) => patch('udpPps', e.target.value)} disabled={!form.udpFloodEnabled} />
        </FormField>
        <FormField label={t('security.udpNewFlows')}>
          <Input type="number" min={0} max={1000000} value={form.udpNewFlows} onChange={(e) => patch('udpNewFlows', e.target.value)} disabled={!form.udpFloodEnabled} />
        </FormField>
        <FormField label={t('security.udpWindowSecs')}>
          <Input type="number" min={1} max={86400} value={form.udpWindowSecs} onChange={(e) => patch('udpWindowSecs', e.target.value)} disabled={!form.udpFloodEnabled} />
        </FormField>
      </PolicySection>

      <PolicySection title={t('security.categories.nodeCapacity')}>
        <label className="flex items-center gap-2 text-sm">
          <input type="checkbox" checked={form.nodeLimitsEnabled} onChange={(e) => patch('nodeLimitsEnabled', e.target.checked)} />
          <PolicyLabel name={t('security.policies.nodeLimits.name')} tip={t('security.policies.nodeLimits.tip')} />
        </label>
        <FormField label={t('security.maxTunnels')}>
          <Input type="number" min={0} max={1000000} value={form.maxTunnels} onChange={(e) => patch('maxTunnels', e.target.value)} disabled={!form.nodeLimitsEnabled} />
        </FormField>
      </PolicySection>

      <PolicySection title={t('security.categories.protocol')}>
        <label className="flex items-center gap-2 text-sm">
          <input type="checkbox" checked={form.blockHttpOnL4} onChange={(e) => patch('blockHttpOnL4', e.target.checked)} />
          <PolicyLabel name={t('security.policies.blockHttpOnL4.name')} tip={t('security.policies.blockHttpOnL4.tip')} />
        </label>
      </PolicySection>

      <PolicySection title={t('security.categories.access')}>
        <label className="flex items-center gap-2 text-sm">
          <input type="checkbox" checked={form.perTunnelIpEnabled} onChange={(e) => patch('perTunnelIpEnabled', e.target.checked)} />
          <PolicyLabel name={t('security.policies.perTunnelIp.name')} tip={t('security.policies.perTunnelIp.tip')} />
        </label>
        <FormField label={t('security.maxDistinctIps')}>
          <Input type="number" min={0} max={1000000} value={form.maxDistinctIps} onChange={(e) => patch('maxDistinctIps', e.target.value)} disabled={!form.perTunnelIpEnabled} />
        </FormField>
        <FormField label={t('security.maxDistinctIpsWindowSecs')}>
          <Input type="number" min={1} max={86400} value={form.maxDistinctIpsWindowSecs} onChange={(e) => patch('maxDistinctIpsWindowSecs', e.target.value)} disabled={!form.perTunnelIpEnabled} />
        </FormField>

        <label className="flex items-center gap-2 text-sm">
          <input type="checkbox" checked={form.cnResidencyEnabled} onChange={(e) => patch('cnResidencyEnabled', e.target.checked)} />
          <PolicyLabel name={t('security.policies.cnResidency.name')} tip={t('security.policies.cnResidency.tip')} />
        </label>

        <label className="flex items-center gap-2 text-sm">
          <input type="checkbox" checked={form.cnHttpFilingEnabled} onChange={(e) => patch('cnHttpFilingEnabled', e.target.checked)} />
          <PolicyLabel name={t('security.policies.cnHttpFiling.name')} tip={t('security.policies.cnHttpFiling.tip')} />
        </label>

        <label className="flex items-center gap-2 text-sm">
          <input type="checkbox" checked={form.ipAclEnabled} onChange={(e) => patch('ipAclEnabled', e.target.checked)} />
          <PolicyLabel name={t('security.policies.ipAcl.name')} tip={t('security.policies.ipAcl.tip')} />
        </label>
        <FormField label={t('security.ipAllow')}>
          <textarea className="input-field min-h-20 w-full font-mono text-xs" value={form.ipAllow} onChange={(e) => patch('ipAllow', e.target.value)} disabled={!form.ipAclEnabled} />
        </FormField>
        <FormField label={t('security.ipDeny')}>
          <textarea className="input-field min-h-20 w-full font-mono text-xs" value={form.ipDeny} onChange={(e) => patch('ipDeny', e.target.value)} disabled={!form.ipAclEnabled} />
        </FormField>

        {showTrustedProxies ? (
          <FormField label={translateField(t, 'trusted_proxies_cidr')}>
            <textarea className="input-field min-h-20 w-full font-mono text-xs" value={form.trustedProxies} onChange={(e) => patch('trustedProxies', e.target.value)} />
          </FormField>
        ) : null}
      </PolicySection>

      <PolicySection title={t('security.categories.response')}>
        <label className="flex items-center gap-2 text-sm">
          <input type="checkbox" checked={form.onAttackEnabled} onChange={(e) => patch('onAttackEnabled', e.target.checked)} />
          <PolicyLabel name={t('security.policies.onAttack.name')} tip={t('security.policies.onAttack.tip')} />
        </label>
        <FormField label={t('security.onAttackPauseMinutes')}>
          <Input
            type="number"
            min={0}
            max={10080}
            value={form.onAttackPauseMinutes}
            onChange={(e) => patch('onAttackPauseMinutes', e.target.value)}
            disabled={!form.onAttackEnabled}
          />
        </FormField>
        <FormField label={t('security.onAttackReenableCooldownMinutes')}>
          <Input
            type="number"
            min={0}
            max={10080}
            value={form.onAttackReenableCooldownMinutes}
            onChange={(e) => patch('onAttackReenableCooldownMinutes', e.target.value)}
            disabled={!form.onAttackEnabled}
          />
        </FormField>
      </PolicySection>
    </FormStack>
  )
}
