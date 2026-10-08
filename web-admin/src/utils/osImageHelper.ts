const OS_SVG = (file: string) => `/images/logo/os/${file}`

interface OSConfig {
  name: string
  image: string
  keywords: string[]
}

const osConfigs: OSConfig[] = [
  { name: 'AlmaLinux', image: OS_SVG('alma.svg'), keywords: ['alma', 'almalinux'] },
  { name: 'Alpine Linux', image: '/images/logo/os-alpine.webp', keywords: ['alpine'] },
  { name: 'CentOS', image: OS_SVG('centos.svg'), keywords: ['centos'] },
  { name: 'Debian', image: OS_SVG('debian.svg'), keywords: ['debian'] },
  { name: 'Ubuntu', image: OS_SVG('ubuntu.svg'), keywords: ['ubuntu'] },
  { name: 'Windows', image: OS_SVG('windows.svg'), keywords: ['windows', 'win'] },
  { name: 'Arch Linux', image: OS_SVG('arch.svg'), keywords: ['arch', 'archlinux'] },
  { name: 'macOS', image: OS_SVG('macos.svg'), keywords: ['macos', 'darwin'] },
  { name: 'OpenWrt', image: OS_SVG('openwrt.svg'), keywords: ['openwrt'] },
  { name: 'Rocky Linux', image: OS_SVG('rocky.svg'), keywords: ['rocky'] },
  { name: 'Fedora', image: OS_SVG('fedora.svg'), keywords: ['fedora'] },
]

const defaultOSConfig: OSConfig = {
  name: 'Linux',
  image: '/images/logo/linux.svg',
  keywords: ['linux'],
}

function findOSConfig(osString: string): OSConfig {
  if (!osString) return defaultOSConfig
  const normalized = osString.toLowerCase()
  for (const config of osConfigs) {
    for (const keyword of config.keywords) {
      if (normalized.includes(keyword)) return config
    }
  }
  return defaultOSConfig
}

export function getOSImage(osString: string): string {
  return findOSConfig(osString).image
}

export function getOSName(osString: string): string {
  const config = findOSConfig(osString)
  if (config !== defaultOSConfig) return config.name
  if (!osString?.trim()) return 'Unknown'
  return osString.trim().split(/[\s/]/)[0] || 'Unknown'
}
