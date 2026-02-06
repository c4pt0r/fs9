#!/usr/bin/env python3
"""
FS9 Meta Service

Central authentication and namespace management for FS9 multi-tenant deployments.

APIs:
- POST /api/v1/tokens/validate - Validate a JWT token
- POST /api/v1/tokens/refresh - Refresh a JWT token
- Admin APIs for namespace/user management (requires x-fs9-meta-key header)
"""

import os
import time
import secrets
from datetime import datetime, timedelta, timezone
from typing import Optional
from contextlib import asynccontextmanager

from fastapi import FastAPI, HTTPException, Header, Depends
from fastapi.middleware.cors import CORSMiddleware
from pydantic import BaseModel
from jose import jwt, JWTError
from sqlalchemy import create_engine, Column, String, Integer, Boolean, DateTime, ForeignKey, Text
from sqlalchemy.orm import sessionmaker, declarative_base, Session, relationship
from passlib.context import CryptContext


# --- Config ---

JWT_SECRET = os.getenv("FS9_JWT_SECRET", "change-me-in-production")
JWT_ALGORITHM = "HS256"
ADMIN_KEY = os.getenv("FS9_META_KEY", "admin-key-change-me")
DATABASE_URL = os.getenv("FS9_META_DB", "sqlite:///./data/fs9_meta.db")
DEFAULT_TOKEN_TTL = int(os.getenv("FS9_TOKEN_TTL", "86400"))  # 24 hours

# --- Database ---

Base = declarative_base()


class Namespace(Base):
    """A tenant namespace."""
    __tablename__ = "namespaces"
    
    id = Column(String, primary_key=True)
    name = Column(String, unique=True, nullable=False)
    description = Column(Text, default="")
    created_at = Column(DateTime, default=lambda: datetime.now(timezone.utc))
    
    users = relationship("User", back_populates="namespace", cascade="all, delete-orphan")


class User(Base):
    """A user within a namespace."""
    __tablename__ = "users"
    
    id = Column(String, primary_key=True)
    username = Column(String, nullable=False)
    password_hash = Column(String, nullable=True)  # Optional, for API key users
    namespace_id = Column(String, ForeignKey("namespaces.id"), nullable=False)
    roles = Column(String, default="read-write")  # comma-separated: read-only, read-write, admin
    active = Column(Boolean, default=True)
    created_at = Column(DateTime, default=lambda: datetime.now(timezone.utc))
    
    namespace = relationship("Namespace", back_populates="users")
    
    @property
    def role_list(self):
        return [r.strip() for r in self.roles.split(",") if r.strip()]


class Token(Base):
    """Issued tokens (for revocation tracking)."""
    __tablename__ = "tokens"
    
    id = Column(String, primary_key=True)
    user_id = Column(String, ForeignKey("users.id"), nullable=False)
    jti = Column(String, unique=True, nullable=False)  # JWT ID
    expires_at = Column(DateTime, nullable=False)
    revoked = Column(Boolean, default=False)
    created_at = Column(DateTime, default=lambda: datetime.now(timezone.utc))


# --- Password Hashing ---

pwd_context = CryptContext(schemes=["bcrypt"], deprecated="auto")


# --- Pydantic Models ---

class ValidateRequest(BaseModel):
    token: str


class ValidateResponse(BaseModel):
    valid: bool
    user_id: Optional[str] = None
    namespace: Optional[str] = None
    roles: list[str] = []
    expires_at: Optional[str] = None
    error: Optional[str] = None


class RefreshRequest(BaseModel):
    token: str
    ttl_seconds: Optional[int] = None


class RefreshResponse(BaseModel):
    token: str
    expires_at: str


class NamespaceCreate(BaseModel):
    name: str
    description: str = ""


class NamespaceResponse(BaseModel):
    id: str
    name: str
    description: str
    created_at: str
    user_count: int = 0


class UserCreate(BaseModel):
    username: str
    password: Optional[str] = None
    roles: list[str] = ["read-write"]


class UserResponse(BaseModel):
    id: str
    username: str
    namespace: str
    roles: list[str]
    active: bool
    created_at: str


class TokenCreate(BaseModel):
    user_id: str
    ttl_seconds: int = DEFAULT_TOKEN_TTL


class TokenResponse(BaseModel):
    token: str
    user_id: str
    namespace: str
    roles: list[str]
    expires_at: str


# --- App Setup ---

engine = None
SessionLocal = None


@asynccontextmanager
async def lifespan(app: FastAPI):
    global engine, SessionLocal
    
    # Ensure data directory exists
    db_path = DATABASE_URL.replace("sqlite:///", "")
    if db_path.startswith("./"):
        db_path = db_path[2:]
    db_dir = os.path.dirname(db_path)
    if db_dir:
        os.makedirs(db_dir, exist_ok=True)
    
    engine = create_engine(DATABASE_URL, connect_args={"check_same_thread": False})
    SessionLocal = sessionmaker(autocommit=False, autoflush=False, bind=engine)
    Base.metadata.create_all(bind=engine)
    
    # Create default namespace if not exists
    with SessionLocal() as db:
        default_ns = db.query(Namespace).filter(Namespace.name == "default").first()
        if not default_ns:
            default_ns = Namespace(id=secrets.token_hex(16), name="default", description="Default namespace")
            db.add(default_ns)
            db.commit()
            print("Created default namespace")
    
    yield


app = FastAPI(
    title="FS9 Meta Service",
    description="Authentication and namespace management for FS9",
    version="0.1.0",
    lifespan=lifespan
)

app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],
    allow_methods=["*"],
    allow_headers=["*"],
)


def get_db():
    db = SessionLocal()
    try:
        yield db
    finally:
        db.close()


def require_admin(x_fs9_meta_key: str = Header(None)):
    """Verify admin key for management operations."""
    if not x_fs9_meta_key or x_fs9_meta_key != ADMIN_KEY:
        raise HTTPException(status_code=403, detail="Invalid or missing admin key")
    return True


# --- JWT Helpers ---

def create_token(user: User, ttl_seconds: int = DEFAULT_TOKEN_TTL) -> tuple[str, datetime]:
    """Create a JWT token for a user."""
    now = datetime.now(timezone.utc)
    expires = now + timedelta(seconds=ttl_seconds)
    
    payload = {
        "sub": user.id,
        "ns": user.namespace.name,
        "roles": user.role_list,
        "iat": int(now.timestamp()),
        "exp": int(expires.timestamp()),
        "jti": secrets.token_hex(16),
    }
    
    token = jwt.encode(payload, JWT_SECRET, algorithm=JWT_ALGORITHM)
    return token, expires


def decode_token(token: str, verify_exp: bool = True) -> dict:
    """Decode and verify a JWT token."""
    options = {"verify_exp": verify_exp}
    return jwt.decode(token, JWT_SECRET, algorithms=[JWT_ALGORITHM], options=options)


# --- Health ---

@app.get("/health")
async def health():
    return {"status": "ok", "service": "fs9-meta"}


# --- Token Validation ---

@app.post("/api/v1/tokens/validate", response_model=ValidateResponse)
async def validate_token(req: ValidateRequest, db: Session = Depends(get_db)):
    """Validate a JWT token."""
    try:
        payload = decode_token(req.token)
        
        # Check if token is revoked
        jti = payload.get("jti")
        if jti:
            revoked = db.query(Token).filter(Token.jti == jti, Token.revoked == True).first()
            if revoked:
                return ValidateResponse(valid=False, error="Token has been revoked")
        
        # Check if user still exists and is active
        user_id = payload.get("sub")
        user = db.query(User).filter(User.id == user_id).first()
        if not user:
            return ValidateResponse(valid=False, error="User not found")
        if not user.active:
            return ValidateResponse(valid=False, error="User is inactive")
        
        exp = payload.get("exp")
        expires_at = datetime.fromtimestamp(exp, tz=timezone.utc).isoformat() if exp else None
        
        return ValidateResponse(
            valid=True,
            user_id=user_id,
            namespace=payload.get("ns"),
            roles=payload.get("roles", []),
            expires_at=expires_at,
        )
    except JWTError as e:
        return ValidateResponse(valid=False, error=str(e))
    except Exception as e:
        return ValidateResponse(valid=False, error=f"Validation error: {e}")


@app.post("/api/v1/tokens/refresh", response_model=RefreshResponse)
async def refresh_token(req: RefreshRequest, db: Session = Depends(get_db)):
    """Refresh a JWT token (can be expired within grace period)."""
    try:
        # Allow expired tokens for refresh (7 day grace period)
        payload = decode_token(req.token, verify_exp=False)
        
        exp = payload.get("exp", 0)
        now = time.time()
        grace_period = 7 * 24 * 60 * 60  # 7 days
        
        if exp + grace_period < now:
            raise HTTPException(status_code=401, detail="Token too old to refresh")
        
        # Check user
        user_id = payload.get("sub")
        user = db.query(User).filter(User.id == user_id).first()
        if not user or not user.active:
            raise HTTPException(status_code=401, detail="User not found or inactive")
        
        # Create new token
        ttl = req.ttl_seconds or DEFAULT_TOKEN_TTL
        new_token, expires = create_token(user, ttl)
        
        return RefreshResponse(
            token=new_token,
            expires_at=expires.isoformat(),
        )
    except JWTError as e:
        raise HTTPException(status_code=401, detail=f"Invalid token: {e}")


# --- Admin: Namespace Management ---

@app.post("/api/v1/admin/namespaces", response_model=NamespaceResponse)
async def create_namespace(
    data: NamespaceCreate,
    _: bool = Depends(require_admin),
    db: Session = Depends(get_db)
):
    """Create a new namespace."""
    # Validate name
    import re
    if not re.match(r'^[a-z0-9][a-z0-9_-]*$', data.name):
        raise HTTPException(400, "Invalid namespace name (lowercase alphanumeric, hyphens, underscores)")
    
    if db.query(Namespace).filter(Namespace.name == data.name).first():
        raise HTTPException(400, "Namespace already exists")
    
    ns = Namespace(
        id=secrets.token_hex(16),
        name=data.name,
        description=data.description,
    )
    db.add(ns)
    db.commit()
    db.refresh(ns)
    
    return NamespaceResponse(
        id=ns.id,
        name=ns.name,
        description=ns.description,
        created_at=ns.created_at.isoformat(),
        user_count=0,
    )


@app.get("/api/v1/admin/namespaces", response_model=list[NamespaceResponse])
async def list_namespaces(_: bool = Depends(require_admin), db: Session = Depends(get_db)):
    """List all namespaces."""
    namespaces = db.query(Namespace).all()
    return [
        NamespaceResponse(
            id=ns.id,
            name=ns.name,
            description=ns.description,
            created_at=ns.created_at.isoformat(),
            user_count=len(ns.users),
        )
        for ns in namespaces
    ]


@app.get("/api/v1/admin/namespaces/{name}", response_model=NamespaceResponse)
async def get_namespace(name: str, _: bool = Depends(require_admin), db: Session = Depends(get_db)):
    """Get namespace details."""
    ns = db.query(Namespace).filter(Namespace.name == name).first()
    if not ns:
        raise HTTPException(404, "Namespace not found")
    
    return NamespaceResponse(
        id=ns.id,
        name=ns.name,
        description=ns.description,
        created_at=ns.created_at.isoformat(),
        user_count=len(ns.users),
    )


@app.delete("/api/v1/admin/namespaces/{name}")
async def delete_namespace(name: str, _: bool = Depends(require_admin), db: Session = Depends(get_db)):
    """Delete a namespace and all its users."""
    if name == "default":
        raise HTTPException(400, "Cannot delete default namespace")
    
    ns = db.query(Namespace).filter(Namespace.name == name).first()
    if not ns:
        raise HTTPException(404, "Namespace not found")
    
    db.delete(ns)
    db.commit()
    
    return {"status": "deleted", "name": name}


# --- Admin: User Management ---

@app.post("/api/v1/admin/namespaces/{namespace}/users", response_model=UserResponse)
async def create_user(
    namespace: str,
    data: UserCreate,
    _: bool = Depends(require_admin),
    db: Session = Depends(get_db)
):
    """Create a user in a namespace."""
    ns = db.query(Namespace).filter(Namespace.name == namespace).first()
    if not ns:
        raise HTTPException(404, "Namespace not found")
    
    # Check if user already exists in namespace
    existing = db.query(User).filter(
        User.namespace_id == ns.id,
        User.username == data.username
    ).first()
    if existing:
        raise HTTPException(400, "User already exists in this namespace")
    
    # Validate roles
    valid_roles = {"read-only", "read-write", "admin"}
    for role in data.roles:
        if role not in valid_roles:
            raise HTTPException(400, f"Invalid role: {role}. Valid roles: {valid_roles}")
    
    password_hash = None
    if data.password:
        password_hash = pwd_context.hash(data.password)
    
    user = User(
        id=secrets.token_hex(16),
        username=data.username,
        password_hash=password_hash,
        namespace_id=ns.id,
        roles=",".join(data.roles),
    )
    db.add(user)
    db.commit()
    db.refresh(user)
    
    return UserResponse(
        id=user.id,
        username=user.username,
        namespace=namespace,
        roles=user.role_list,
        active=user.active,
        created_at=user.created_at.isoformat(),
    )


@app.get("/api/v1/admin/namespaces/{namespace}/users", response_model=list[UserResponse])
async def list_users(namespace: str, _: bool = Depends(require_admin), db: Session = Depends(get_db)):
    """List users in a namespace."""
    ns = db.query(Namespace).filter(Namespace.name == namespace).first()
    if not ns:
        raise HTTPException(404, "Namespace not found")
    
    return [
        UserResponse(
            id=u.id,
            username=u.username,
            namespace=namespace,
            roles=u.role_list,
            active=u.active,
            created_at=u.created_at.isoformat(),
        )
        for u in ns.users
    ]


@app.delete("/api/v1/admin/users/{user_id}")
async def delete_user(user_id: str, _: bool = Depends(require_admin), db: Session = Depends(get_db)):
    """Delete a user."""
    user = db.query(User).filter(User.id == user_id).first()
    if not user:
        raise HTTPException(404, "User not found")
    
    db.delete(user)
    db.commit()
    
    return {"status": "deleted", "user_id": user_id}


# --- Admin: Token Generation ---

@app.post("/api/v1/admin/tokens", response_model=TokenResponse)
async def generate_token(
    data: TokenCreate,
    _: bool = Depends(require_admin),
    db: Session = Depends(get_db)
):
    """Generate a token for a user (admin only)."""
    user = db.query(User).filter(User.id == data.user_id).first()
    if not user:
        raise HTTPException(404, "User not found")
    if not user.active:
        raise HTTPException(400, "User is inactive")
    
    token, expires = create_token(user, data.ttl_seconds)
    
    return TokenResponse(
        token=token,
        user_id=user.id,
        namespace=user.namespace.name,
        roles=user.role_list,
        expires_at=expires.isoformat(),
    )


# --- Run ---

if __name__ == "__main__":
    import uvicorn
    port = int(os.getenv("FS9_META_PORT", "9998"))
    uvicorn.run(app, host="0.0.0.0", port=port)
