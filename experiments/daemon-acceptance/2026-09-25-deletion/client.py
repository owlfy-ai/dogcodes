"""Local RPC helper for isolated deletion acceptance. Never prints the service token."""
import datetime,json,pathlib,socket,struct,uuid

def uid(prefix):
    alphabet='0123456789ABCDEFGHJKMNPQRSTVWXYZ'; n=uuid.uuid4().int; chars=''
    for _ in range(26): chars=alphabet[n&31]+chars; n>>=5
    return prefix+'_'+chars

class Client:
    def __init__(self,directory): self.directory=pathlib.Path(directory)
    def connect(self,method,args):
        e=json.loads((self.directory/'service.json').read_text()); host,port=e['address'].split(':')
        assert host=='127.0.0.1'
        s=socket.create_connection((host,int(port)),timeout=30)
        data=json.dumps(dict(version=1,token=e['token'],method=method,args=args)).encode()
        s.sendall(struct.pack('!I',len(data))+data);return s
    @staticmethod
    def read(s):
        def exact(n):
            result=b''
            while len(result)<n:
                chunk=s.recv(n-len(result))
                if not chunk: raise ConnectionError('socket closed')
                result+=chunk
            return result
        n=struct.unpack('!I',exact(4))[0];assert n<=64*1024*1024
        return json.loads(exact(n))
    def call(self,method,args=None):
        with self.connect(method,args) as s: reply=self.read(s)
        if 'Err' in reply: raise RuntimeError(reply['Err'])
        return reply['Ok']
    def envelope(self,kind,data,version=None):
        command=uid('cmd')
        return dict(schemaVersion=1,commandId=command,idempotencyKey=command,actor={'kind':'local_user'},expectedStreamVersion=version,issuedAt=datetime.datetime.now(datetime.timezone.utc).isoformat(),payload=dict(type=kind,data=data))
    def execute(self,kind,data,version=None): return self.call('execute',[self.envelope(kind,data,version)])
    def task(self,task,deleted=False): return next(t for t in self.call('bootstrap')['deletedTasks' if deleted else 'tasks'] if t['taskId']==task)
