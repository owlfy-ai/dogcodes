// Personal Coding Plan project authentication, following the installed Desktop
// contract. Only an existing native Key is used; no Key or credential is stored.
function locationFrom(value) {
  if (!value || typeof value !== 'object') return undefined;
  const keys = ['organizationId', 'projectId', 'apiKeyId'];
  if (!keys.every(key => typeof value[key] === 'string' && value[key].trim())) return undefined;
  return Object.fromEntries(keys.map(key => [key, value[key]]));
}

async function resolvePersonalToken({ account, baseUrl, version, request, assertCurrent }) {
  if (!account.current || !account.loginToken || !account.accountId) throw new Error('project_token_login_required');
  const login = account.loginToken.replace(/^Bearer\s+/i, '');
  const read = async (url, options) => {
    assertCurrent();
    const result = await request(url, options);
    assertCurrent();
    return result;
  };
  const requireData = result => {
    if (result.status !== 'ok') throw new Error(result.status === 'credential-failed'
      ? 'project_token_login_required' : 'project_token_unavailable');
    return result.data;
  };
  const customer = requireData(await read(`${baseUrl}/api/biz/customer/getCustomerInfo`, {
    method: 'GET', headers: { Authorization: login },
  }));
  if (!Array.isArray(customer?.organizations)) throw new Error('project_token_invalid_response');
  const organizations = customer.organizations.map(org => ({ ...org,
    projects: Array.isArray(org.projects) ? org.projects.filter(project =>
      typeof project.projectId === 'string' && project.projectId.trim()
      && String(project.projectType ?? '').trim() !== '2') : [],
  })).filter(org => typeof org.organizationId === 'string' && org.organizationId.trim() && org.projects.length);
  const cached = locationFrom(account.projectLocation);
  let org, project;
  if (cached) {
    org = organizations.find(item => item.organizationId === cached.organizationId);
    project = org?.projects.find(item => item.projectId === cached.projectId);
    // A cached personal location cannot authorize a Team project or another scope.
    if (!project) throw new Error('project_token_project_unavailable');
  } else {
    org = organizations.find(item => item.organizationName?.includes('默认机构')) ?? organizations[0];
    project = org?.projects.find(item => item.projectName?.includes('默认项目')) ?? org?.projects[0];
    if (!project) throw new Error('project_token_project_unavailable');
  }
  const keysUrl = `${baseUrl}/api/biz/v1/organization/${encodeURIComponent(org.organizationId)}`
    + `/projects/${encodeURIComponent(project.projectId)}/api_keys`;
  const findKey = async () => {
    const keys = requireData(await read(keysUrl, { method: 'GET', headers: {
      Authorization: account.family === 'zai' ? `Bearer ${login}` : login,
    } }));
    if (!Array.isArray(keys)) throw new Error('project_token_invalid_response');
    const key = keys.find(item => item?.name === 'zcode-api-key' && item.keyType !== 2
      && typeof item.apiKey === 'string' && item.apiKey.trim());
    if (!key) throw new Error('project_token_key_required');
    return key.apiKey;
  };
  let apiKeyId = cached?.apiKeyId ?? await findKey();
  const issue = id => read(`${keysUrl}/${encodeURIComponent(id)}/access_tokens`, {
    method: 'POST', headers: { Authorization: `Bearer ${login}`, 'Content-Type': 'application/json' },
    body: JSON.stringify({ clientType: 'zcode', clientVersion: version }),
  });
  const started = Date.now();
  let response = await issue(apiKeyId);
  if (cached && response.httpStatus === 404) {
    apiKeyId = await findKey();
    response = await issue(apiKeyId);
  }
  const data = requireData(response);
  if (!data || (data.enable !== undefined && data.enable !== true)
    || (data.enabled !== undefined && data.enabled !== true)
    || data.tokenType !== 'Bearer' || typeof data.accessToken !== 'string' || !data.accessToken.trim()
    || !Number.isFinite(data.expiresIn) || data.expiresIn <= 0
    || !Number.isFinite(data.expiresAt) || data.expiresAt <= 0
    || Math.min(data.expiresAt * 1000, started + data.expiresIn * 1000) <= Date.now()) {
    throw new Error('project_token_invalid_response');
  }
  return { apiKey: data.accessToken, apiKeyId,
    location: { organizationId: org.organizationId, projectId: project.projectId, apiKeyId } };
}

module.exports = { resolvePersonalToken, locationFrom };
