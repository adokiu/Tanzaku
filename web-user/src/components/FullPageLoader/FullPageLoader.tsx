import './FullPageLoader.css'

export function FullPageLoader() {
  return (
    <main className="full-page-loader ambient-surface" aria-busy="true" aria-live="polite">
      <div className="full-page-loader__ring" aria-hidden>
        <span />
        <span />
      </div>
    </main>
  )
}
