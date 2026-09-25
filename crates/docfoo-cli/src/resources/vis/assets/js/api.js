/** Fetch helpers for the resource browser API. */

async function request(url, options) {
  const response = await fetch(url, options);
  let data = null;
  try {
    data = await response.json();
  } catch {
    data = null;
  }
  if (!response.ok) {
    const error = new Error(data?.error || `${response.status} ${response.statusText}`);
    error.status = response.status;
    error.data = data;
    throw error;
  }
  return data;
}

function post(url, body) {
  return request(url, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
}

export function getState() {
  return request('/api/state');
}

export function getLevel(rel = '') {
  return request(`/api/resources?rel=${encodeURIComponent(rel)}`);
}

export function getResource(rel) {
  return request(`/api/resource?path=${encodeURIComponent(rel)}`);
}

export function getNotes(rel) {
  return request(`/api/notes?path=${encodeURIComponent(rel)}`);
}

export function saveNote(rel, note) {
  return post('/api/notes', { rel, note });
}

export function deleteNote(rel, id) {
  return post('/api/notes/delete', { rel, id });
}
