import ReactDOM from 'react-dom/client'
import '@/i18n'
import { applyTypographyVariables } from '@/styles/typography'
import App from './app'

applyTypographyVariables()

ReactDOM.createRoot(document.getElementById('root')!).render(<App />)
