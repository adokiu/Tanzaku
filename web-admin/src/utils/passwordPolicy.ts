/** 与 tz-board store::validate_password 一致 */
export function validatePassword(password: string): string | null {
  if (password.length < 6 || password.length > 32) {
    return 'passwordPolicy.length'
  }
  let classes = 0
  if (/[a-z]/.test(password)) classes += 1
  if (/[A-Z]/.test(password)) classes += 1
  if (/[0-9]/.test(password)) classes += 1
  if (/[^a-zA-Z0-9]/.test(password)) classes += 1
  if (classes < 2) {
    return 'passwordPolicy.classes'
  }
  return null
}
