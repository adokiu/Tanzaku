import { useState, type FormEvent } from 'react'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import apiClient from '@/api/client'
import { usePagedList } from '@/api/page'
import { Button } from '@/components/Button'
import { DataTable, type Column } from '@/components/DataTable'

type User = { id: string; email: string; role: string; status: string; last_login_at: string | null; created_at: string }

export default function UsersPage() {
  const queryClient = useQueryClient()
  const [email, setEmail] = useState('')
  const [password, setPassword] = useState('')
  const [message, setMessage] = useState('')
  const users = usePagedList<User>(['admin-users'], '/v1/admin/users')
  const create = useMutation({
    mutationFn: () => apiClient.post('/v1/admin/users', { email, password }),
    onSuccess: async () => {
      setEmail('')
      setPassword('')
      setMessage('用户已创建')
      await queryClient.invalidateQueries({ queryKey: ['admin-users'] })
    },
    onError: (error) => setMessage(apiError(error)),
  })
  const columns: Column<User>[] = [
    { key: 'email', title: '邮箱' },
    { key: 'status', title: '状态' },
    { key: 'last_login_at', title: '最近登录' },
    { key: 'created_at', title: '创建时间' },
  ]

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    setMessage('')
    create.mutate()
  }

  return <section className="page-container">
    <header className="page-header"><h1 className="page-title">用户管理</h1></header>
    <form className="page-card mb-5 grid gap-4 p-5 md:grid-cols-3" onSubmit={submit}>
      <label><span className="apple-label">用户邮箱</span><input className="apple-input w-full" type="email" value={email} onChange={(event) => setEmail(event.target.value)} required /></label>
      <label><span className="apple-label">初始密码</span><input className="apple-input w-full" type="password" autoComplete="new-password" minLength={12} value={password} onChange={(event) => setPassword(event.target.value)} required /></label>
      <div className="self-end"><Button type="submit" loading={create.isPending}>创建用户</Button></div>
      {message && <p className="text-sm text-muted-foreground md:col-span-3" role="status">{message}</p>}
    </form>
    {users.query.isError ? <p className="page-card p-5 text-sm text-apple-red" role="alert">{apiError(users.query.error)}</p> : <DataTable columns={columns} data={users.items} rowKey={(row) => row.id} loading={users.query.isPending} pagination={users.pagination} onPageChange={users.setPage} onSizeChange={users.setSize} />}
  </section>
}

function apiError(error: unknown): string {
  if (typeof error === 'object' && error !== null && 'response' in error) {
    const response = (error as { response?: { data?: { error?: string } } }).response
    if (response?.data?.error) return response.data.error
  }
  return '无法连接 board'
}
