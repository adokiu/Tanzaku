export default {
  darkMode: ['class'],
  content: ['./*.html', './src/**/*.{ts,tsx}'],
  theme: {
    extend: {
      fontFamily: {
        sans: ['-apple-system', 'BlinkMacSystemFont', "'SF Pro Text'", "'PingFang SC'", "'Microsoft YaHei'", 'sans-serif'],
        display: ['-apple-system', 'BlinkMacSystemFont', "'SF Pro Display'", "'PingFang SC'", 'sans-serif'],
      },
      colors: {
        border: 'hsl(var(--border))',
        input: 'hsl(var(--input))',
        background: 'hsl(var(--background))',
        foreground: 'hsl(var(--foreground))',
        muted: { DEFAULT: 'hsl(var(--muted))', foreground: 'hsl(var(--muted-foreground))' },
        apple: { blue: '#087ed1', green: '#34c759', red: '#ff3b30', gray: { 100: '#f5f5f7', 200: '#e8e8ed', 400: '#aeaeb2', 600: '#6e6e73', 700: '#48484a' } },
      },
      borderRadius: { xl: '1rem', '2xl': '1.25rem' },
      boxShadow: { apple: '0 4px 24px rgba(0,0,0,.04)', 'apple-hover': '0 12px 40px rgba(0,0,0,.12)' },
      backdropBlur: { apple: '20px' },
    },
  },
  plugins: [],
}
