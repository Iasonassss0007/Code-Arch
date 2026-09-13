from flask import Flask

from db import find_users

app = Flask(__name__)


@app.route('/api/users')
def users():
    return find_users()
