import OAuthCallbackClient from "../auth/callback/oauth-callback-client"

export default async function OAuthCallbackPage({ searchParams }: { searchParams: Promise<{ code?: string; error?: string; error_description?: string }> }) {
  const { code, error, error_description } = await searchParams
  return <OAuthCallbackClient hasCode={Boolean(code)} error={error_description || error || ""} />
}
