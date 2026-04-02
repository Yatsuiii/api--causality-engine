"""ACE Desktop – FastAPI sidecar backend."""

import uvicorn
from fastapi import FastAPI
from fastapi.middleware.cors import CORSMiddleware

from routes.scenarios import router as scenarios_router
from routes.runner import router as runner_router
from routes.environments import router as env_router
from routes.history import router as history_router

app = FastAPI(title="ACE Desktop Backend", version="0.1.0")

app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],
    allow_methods=["*"],
    allow_headers=["*"],
)

app.include_router(scenarios_router, prefix="/api/scenarios", tags=["scenarios"])
app.include_router(runner_router, prefix="/api/runner", tags=["runner"])
app.include_router(env_router, prefix="/api/environments", tags=["environments"])
app.include_router(history_router, prefix="/api/history", tags=["history"])


@app.get("/api/health")
def health():
    return {"status": "ok"}


@app.get("/api/workspace")
def get_workspace():
    from services.storage import get_workspace_dir
    return {"workspace": str(get_workspace_dir())}


@app.post("/api/workspace")
def set_workspace(body: dict):
    from services.storage import set_workspace_dir
    set_workspace_dir(body["path"])
    return {"workspace": body["path"]}


if __name__ == "__main__":
    uvicorn.run(app, host="127.0.0.1", port=18710)
