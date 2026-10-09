import { useEffect, useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Copy, Eye, EyeOff, RefreshCw } from 'lucide-react'
import apiClient from '@/api/client'
import { Button } from '@/components/Button'
import { FormField, FormStack } from '@/components/FormField'
import { Input } from '@/components/Input'
import { Modal } from '@/components/Modal/Modal'
import { Select } from '@/components/Select/Select'
import { translateApiError } from '@/i18n/apiError'
import { toast } from '@/stores/toast'

export const GITHUB_REPO = 'adokiu/Tanzaku'
const RAW_BASE = `https://raw.githubusercontent.com/${GITHUB_REPO}/main`
const LOG_LEVELS = ['error', 'warn', 'info', 'debug', 'trace'] as const

type AgentToken = { token: string | null; token_prefix: string }

export interface InstallDialogProps {
  open: boolean
  onClose: () => void
  /** 安装 server（节点）还是 client */
  role: 'client' | 'server'
  /** 节点 / Client 名称，仅展示 */
  name: string
  /** GET 读取、POST 重置 token 的接口路径 */
  tokenEndpoint: string
  /** Board 地址默认值：server 用管理端地址，client 用用户端地址 */
  defaultBoard: string
  /** 刚创建时直接传入 token，省一次请求 */
  initialToken?: string | null
}

function shQuote(value: string) {
  return `'${value.replace(/'/g, `'\\''`)}'`
}

function psQuote(value: string) {
  // 整条命令放在 cmd 的双引号里，值用单引号并去掉双引号即可。
  return `'${value.replace(/"/g, '').replace(/'/g, "''")}'`
}

function withProxy(proxy: string, url: string) {
  const trimmed = proxy.trim().replace(/\/+$/, '')
  return trimmed ? `${trimmed}/${url}` : url
}

export function InstallDialog({ open, onClose, role, name, tokenEndpoint, defaultBoard, initialToken }: InstallDialogProps) {
  const { t } = useTranslation()
  const [token, setToken] = useState<AgentToken | null>(null)
  const [loading, setLoading] = useState(false)
  const [resetting, setResetting] = useState(false)
  const [showToken, setShowToken] = useState(false)
  const [resetConfirmOpen, setResetConfirmOpen] = useState(false)
  const [board, setBoard] = useState(defaultBoard)
  const [version, setVersion] = useState('')
  const [githubProxy, setGithubProxy] = useState('')
  const [installDir, setInstallDir] = useState('')
  const [serviceName, setServiceName] = useState('')
  const [logLevel, setLogLevel] = useState<string>('info')
  const [platform, setPlatform] = useState<'linux' | 'windows'>('linux')
  const [downloader, setDownloader] = useState<'curl' | 'wget'>('curl')

  useEffect(() => {
    if (!open) return
    setBoard(defaultBoard)
    setShowToken(false)
    if (initialToken) {
      setToken({ token: initialToken, token_prefix: initialToken.slice(0, 12) })
      return
    }
    let cancelled = false
    setLoading(true)
    apiClient
      .get(tokenEndpoint)
      .then((response) => {
        if (!cancelled) setToken(response.data as AgentToken)
      })
      .catch((error) => {
        if (!cancelled) toast.error(translateApiError(t, error))
      })
      .finally(() => {
        if (!cancelled) setLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [open, tokenEndpoint, defaultBoard, initialToken, t])

  async function resetToken() {
    setResetConfirmOpen(false)
    setResetting(true)
    try {
      const response = await apiClient.post(tokenEndpoint)
      setToken(response.data as AgentToken)
      setShowToken(true)
      toast.success(t('install.resetSuccess'))
    } catch (error) {
      toast.error(translateApiError(t, error))
    } finally {
      setResetting(false)
    }
  }

  const tokenValue = token?.token ?? ''
  const tokenPlaceholder = '<TOKEN>'

  const commands = useMemo(() => {
    const tokenArg = tokenValue || tokenPlaceholder
    const boardArg = board.trim() || '<BOARD_URL>'
    const shArgs = ['-r', role, '-e', shQuote(boardArg), '-t', shQuote(tokenArg)]
    const psArgs = ['-Role', role, '-Endpoint', psQuote(boardArg), '-Token', psQuote(tokenArg)]
    if (version.trim()) {
      shArgs.push('--version', shQuote(version.trim()))
      psArgs.push('-Version', psQuote(version.trim()))
    }
    if (githubProxy.trim()) {
      shArgs.push('--github-proxy', shQuote(githubProxy.trim()))
      psArgs.push('-GithubProxy', psQuote(githubProxy.trim()))
    }
    if (installDir.trim()) {
      shArgs.push('--install-dir', shQuote(installDir.trim()))
      psArgs.push('-InstallDir', psQuote(installDir.trim()))
    }
    if (serviceName.trim()) {
      shArgs.push('--service-name', shQuote(serviceName.trim()))
      psArgs.push('-ServiceName', psQuote(serviceName.trim()))
    }
    if (logLevel !== 'info') {
      shArgs.push('--log-level', logLevel)
      psArgs.push('-LogLevel', logLevel)
    }
    const shUrl = withProxy(githubProxy, `${RAW_BASE}/install.sh`)
    const psUrl = withProxy(githubProxy, `${RAW_BASE}/install.ps1`)
    const fetcher = downloader === 'curl' ? `curl -fsSL ${shUrl}` : `wget -qO- ${shUrl}`
    return {
      linux: `${fetcher} | sudo sh -s -- ${shArgs.join(' ')}`,
      windows: `powershell -ExecutionPolicy Bypass -Command "& ([scriptblock]::Create((irm '${psUrl}'))) ${psArgs.join(' ')}"`,
    }
  }, [board, downloader, githubProxy, installDir, logLevel, role, serviceName, tokenValue, version])

  async function copy(text: string) {
    try {
      await navigator.clipboard.writeText(text)
      toast.success(t('install.copied'))
    } catch {
      toast.error(t('install.copyFailed'))
    }
  }

  const command = platform === 'linux' ? commands.linux : commands.windows

  return (
    <>
      <Modal
        open={open}
        onClose={onClose}
        title={t('install.title', { name })}
        width={720}
        footer={<Button onClick={onClose}>{t('common.cancel')}</Button>}
      >
        <FormStack>
          <FormField label={t('install.token')}>
            <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
              <Input
                readOnly
                className="font-mono text-xs"
                style={{ flex: 1 }}
                value={loading ? '…' : tokenValue ? (showToken ? tokenValue : `${token?.token_prefix ?? ''}${'•'.repeat(20)}`) : ''}
                placeholder={token && !tokenValue ? t('install.tokenMissing') : ''}
              />
              <Button variant="secondary" size="sm" disabled={!tokenValue} onClick={() => setShowToken((value) => !value)}>
                {showToken ? <EyeOff size={14} /> : <Eye size={14} />}
              </Button>
              <Button variant="secondary" size="sm" disabled={!tokenValue} onClick={() => void copy(tokenValue)}>
                <Copy size={14} />
              </Button>
              <Button variant="secondary" size="sm" loading={resetting} onClick={() => setResetConfirmOpen(true)}>
                <RefreshCw size={14} />
                {t('install.resetToken')}
              </Button>
            </div>
            {token && !tokenValue ? (
              <p style={{ fontSize: 12, color: 'hsl(var(--destructive))', margin: '6px 0 0' }}>{t('install.tokenMissingHint')}</p>
            ) : null}
          </FormField>

          <FormField label={t('install.board')} required hint={t(role === 'server' ? 'install.boardHintServer' : 'install.boardHintClient')}>
            <Input value={board} onChange={(event) => setBoard(event.target.value)} placeholder="https://board.example.com:9001" />
          </FormField>

          <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 12 }}>
            <FormField label={t('install.version')} hint={t('install.versionHint')}>
              <Input value={version} onChange={(event) => setVersion(event.target.value)} placeholder={t('install.latest')} />
            </FormField>
            <FormField label={t('install.githubProxy')} hint={t('install.githubProxyHint')}>
              <Input value={githubProxy} onChange={(event) => setGithubProxy(event.target.value)} placeholder="https://ghfast.top" />
            </FormField>
            <FormField label={t('install.installDir')}>
              <Input
                value={installDir}
                onChange={(event) => setInstallDir(event.target.value)}
                placeholder={platform === 'windows' ? `%ProgramFiles%\\Tanzaku\\${role}` : `/opt/tanzaku-${role}`}
              />
            </FormField>
            <FormField label={t('install.serviceName')}>
              <Input value={serviceName} onChange={(event) => setServiceName(event.target.value)} placeholder={`tanzaku-${role}`} />
            </FormField>
            <FormField label={t('install.logLevel')}>
              <Select
                value={logLevel}
                options={LOG_LEVELS.map((level) => ({ label: level, value: level }))}
                onChange={(value) => setLogLevel(String(value))}
              />
            </FormField>
            <FormField label={t('install.platform')}>
              <div style={{ display: 'flex', gap: 8 }}>
                <Select
                  value={platform}
                  options={[
                    { label: 'Linux / macOS', value: 'linux' },
                    { label: 'Windows', value: 'windows' },
                  ]}
                  onChange={(value) => setPlatform(value as 'linux' | 'windows')}
                />
                {platform === 'linux' ? (
                  <Select
                    value={downloader}
                    options={[
                      { label: 'curl', value: 'curl' },
                      { label: 'wget', value: 'wget' },
                    ]}
                    onChange={(value) => setDownloader(value as 'curl' | 'wget')}
                  />
                ) : null}
              </div>
            </FormField>
          </div>

          <FormField label={t('install.command')} hint={t(platform === 'linux' ? 'install.commandHintLinux' : 'install.commandHintWindows')}>
            <pre
              style={{
                margin: 0,
                padding: 12,
                borderRadius: 8,
                background: 'hsl(var(--muted))',
                fontSize: 12,
                lineHeight: 1.6,
                whiteSpace: 'pre-wrap',
                wordBreak: 'break-all',
                fontFamily: 'ui-monospace, SFMono-Regular, Menlo, monospace',
              }}
            >
              {command}
            </pre>
            <div style={{ marginTop: 8, display: 'flex', justifyContent: 'flex-end' }}>
              <Button size="sm" disabled={!tokenValue} onClick={() => void copy(command)}>
                <Copy size={14} />
                {t('install.copyCommand')}
              </Button>
            </div>
          </FormField>
        </FormStack>
      </Modal>
      <Modal
        open={resetConfirmOpen}
        onClose={() => setResetConfirmOpen(false)}
        title={t('install.resetToken')}
        confirmMode
        confirmText={t('install.resetToken')}
        cancelText={t('common.cancel')}
        onConfirm={() => void resetToken()}
      >
        {t('install.resetConfirm', { name })}
      </Modal>
    </>
  )
}
