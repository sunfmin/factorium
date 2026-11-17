// What a probe of a DSH model loads in its headless profile, and nothing
// else: the model it names, set on each request as a session's plugin sets it,
// and that model printed after the mark factorium reads it by
// (`dsh::ANSWERED`): DSH's headless answer names no model.
export const name = 'factorium-probe'
export const inject = ['deepseekAccount']

export function apply(ctx) {
  ctx.on('agent/request', async (_event, next) => {
    const config = await next()
    const model = process.env.FACTORIUM_DSH_MODEL
    // Ask only for sign-in state; DSH's provider owns the token used for the
    // probe itself. The legacy headless default always chooses the API key.
    const account = await ctx.deepseekAccount.getState()
    const provider = account.status === 'credential-stored' ? 'deepseek-account' : 'deepseek-official'
    const sent = { ...config, provider, ...(model ? { model } : {}) }
    if (sent.model) process.stdout.write(`FACTORIUM_DSH_ANSWERED ${sent.model}\n`)
    return sent
  })
}
